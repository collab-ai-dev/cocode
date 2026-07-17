# TUI Rendering Hardening And Rollback

Status: historical cursor/suspend invariants plus rollback ledger for
`terminal-surface-design.md`. Phase A + B landed and remain in force. Phase C
(inline viewport + native scrollback) was attempted and rolled back after user
testing showed duplicate rendering. The later native-surface migration replaced
the old fullscreen renderer; this document remains as evidence for the cursor
and suspend/resume invariants.

The rollback history is evidence; the cursor and suspend/resume rules in this
document remain production constraints and are now applied through
`surface::terminal::SurfaceFrame` / `SurfaceTerminal`.

## Current Goal

This hardening pass fixed two terminal-layer problems before the native-surface
migration:

1. **Cursor pin**: every draw produces an explicit cursor decision. Focus-gained
   redraws cannot leave the cursor at the status bar or another stale write
   position.
2. **Suspend / resume**: Ctrl+Z on Unix leaves TUI modes, yields to the shell,
   and re-enters the TUI after `fg`.

Native terminal scrollback now uses the custom `surface/*` path documented in
`native-scrollback-architecture.md`; the failed stock-ratatui inline attempt
below should not be revived.

## Landed Architecture

`Tui::draw` owns terminal side effects around native-surface drawing:

```rust
self.surface.draw(&mut self.terminal, state)?;
```

Draw order:

1. Apply a pending resume action, if Ctrl+Z recently returned.
2. Sync the retained viewport / alt-screen overlay placement.
3. Emit finalized history and render the interactive viewport.
4. Apply the cursor claim through `SurfaceTerminal`.

The important constraint is that widgets no longer call
`Frame::set_cursor_position`. Cursor ownership is centralized in
`cursor::compute_cursor`.

## Cursor Rules

- If input is focused and no modal overlay owns the screen, claim the input
  cursor.
- The command palette is the exception: its filter is mirrored into the input
  row, so the cursor follows `/<filter>`.
- Other overlays hide the base input cursor. Future overlay text fields should
  expose their own cursor claim through the surface/focus architecture rather
  than borrowing the base input cursor.
- Empty input still claims a cursor at the first editable column.
- CJK and other wide characters use display width, not byte or char count.
- Future native-surface arbitration keeps one winner in this order: focused
  local surface, active overlay, composer/input, then base viewport. The base
  viewport normally hides the cursor because native scrollback history is not
  editable.

## Suspend / Resume Rules

On Unix, raw mode prevents the terminal driver from turning Ctrl+Z into
SIGTSTP, so the app intercepts the key before keybinding dispatch.

Flow:

1. Leave TUI modes: raw mode off, alt-screen off, bracketed paste/focus-change
   reporting off.
2. Show the cursor on a fresh normal-buffer row.
3. Record a pending resume action.
4. Send `SIGTSTP` to the process group with `libc::kill(0, SIGTSTP)`.
5. After `fg`/SIGCONT, re-enter TUI modes.
6. On the next draw, clear the terminal and force a full repaint.

If suspend setup fails after TUI modes were left, the implementation attempts to
restore TUI modes before returning the error. The app exits on a suspend error
instead of continuing with unknown terminal state.

## Phase C Rollback

The original Phase C tried to render committed messages into terminal native
scrollback with ratatui `Viewport::Inline` and `Terminal::insert_before`. That
was rolled back.

Two approaches failed:

1. **Dynamic inline viewport resize**: stock ratatui 0.30 recomputes inline
   viewport geometry from construction-time state. It cannot reliably keep a
   chrome viewport pinned at the terminal bottom while messages are inserted
   above it.
2. **Fullscreen inline viewport + insert_before**: inserting committed lines
   scrolls visible streaming content into terminal history, then the committed
   version is also inserted. Users saw duplicated turns.

Current behavior after rollback:

| Scenario | Behavior |
|---|---|
| Start session | Enter alt-screen; render header, chat, input, status, overlays. |
| Submit message | Render user + streaming tail, then committed user + assistant in the same fullscreen viewport. |
| Exit coco | Leave alt-screen; prior shell buffer returns. Coco transcript is not left in terminal scrollback. |
| Ctrl+Z / fg | Leave TUI modes for shell use; re-enter and repaint after resume. |
| Focus gained | Redraw and re-apply the cursor pin. |

## Future Terminal Backend Options

If native scrollback and mouse-wheel history become a hard requirement, choose
one explicit backend path:

1. Port a coco-owned custom terminal adapter based on the `codex-rs/tui`
   terminal model, including explicit viewport geometry and committed-history
   insertion.
2. Use raw crossterm with a coco-owned buffer diff.
3. Keep stock ratatui fullscreen and improve in-app transcript/pager UX instead
   of native scrollback. This option was retired when the native-surface
   migration deleted the fullscreen renderer.

Do not reintroduce a partial stock-ratatui inline viewport implementation
without a new design and PTY-level tests for streaming commit, resize, focus,
and suspend/resume.

## Verification

Automated:

- `compute_cursor`: focused empty input, ASCII, CJK, command palette, modal
  overlay hidden, zero-sized layout.
- `SuspendContext::prepare_resume_action`: empty and one-shot pending action.
- Snapshot tests for changed visible UI.

Manual:

1. Empty input cursor appears after `❯ `.
2. Focus lost/gained keeps the cursor in the input, not the status bar.
3. Modal overlays do not show the base input cursor.
4. Command palette cursor tracks the mirrored `/<filter>`.
5. Ctrl+Z drops to a usable shell; `fg` restores and repaints the TUI.
6. Repeat Ctrl+Z/`fg` several times on macOS and Linux terminals.

## Known Limitations

| Scenario | Current behavior |
|---|---|
| External `kill -TSTP $pid` / `kill -STOP $pid` | Not handled; needs a signal handler if required. |
| Native terminal scrollback | Implemented through `surface::controller`, `history_driver`, and `SurfaceTerminal`; keep stock inline viewport disabled. |
| Mouse wheel history | Not available because mouse capture remains disabled and alt-screen has no native transcript scrollback. |
| Long sessions | `state.session.messages` grows linearly; compaction and rewind remain the current pressure valves. |
