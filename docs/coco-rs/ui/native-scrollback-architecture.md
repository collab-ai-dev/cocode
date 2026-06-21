# TUI Native Scrollback Architecture

Status: supporting terminal-backend architecture for `terminal-surface-design.md`.

This document designs the real native-scrollback architecture for `coco-tui`.
It intentionally does not revive the earlier stock-ratatui `Viewport::Inline`
experiment. The target is a retained interactive viewport at the bottom of the
terminal, with finalized chat history written into terminal-native scrollback.

The goal is one long-lived UI implementation, not an MVP followed by a second
rewrite. The implementation phases below are verification gates only. Native
scrollback is not complete until terminal ownership, history emission,
source-backed resize replay, overlay handling, cursor policy, suspend/resume,
and terminal-matrix tests all work together.

## Why This Exists

The current full-screen alt-screen TUI is stable after the rendering hardening
work, but it still has the wrong long-session ergonomics:

- terminal wheel / trackpad scroll does not inspect finalized chat history;
- native terminal selection / copy cannot naturally grab older messages;
- exiting the TUI leaves no transcript in the shell scrollback;
- long tool output is trapped inside app-owned scroll regions;
- app-level chat scrolling, overlays, focus, and cursor state are coupled.

Native scrollback should make finalized history behave like normal terminal
output while keeping the composer, status row, live stream, and overlays
interactive.

The explicit product target is:

- native scrollback is the primary long-session surface;
- native scrollback is the only long-term base surface;
- no temporary native backend is introduced with known architectural gaps;
- every terminal side effect is owned by the surface layer and covered by
  source-backed tests or PTY/manual acceptance checks.

## Codex-RS Reference Findings

The useful reference is not ratatui's stock inline viewport. It is the
purpose-built terminal stack in `codex-rs/tui`:

- `codex-rs/tui/src/cli.rs`: exposes `--no-alt-screen`, described as inline
  mode preserving terminal scrollback.
- `codex-rs/tui/src/tui.rs`: starts without `EnterAlternateScreen`, probes the
  initial cursor position, owns `pending_history_lines`, updates an inline
  viewport to the requested height, and enters alt-screen only for overlay
  surfaces.
- `codex-rs/tui/src/custom_terminal.rs`: forks ratatui `Terminal` so the app
  owns `viewport_area`, `last_known_cursor_pos`, cursor style, visible-history
  accounting, full-screen clear operations, and diff invalidation.
- `codex-rs/tui/src/insert_history.rs`: inserts finalized history above the
  viewport using DECSTBM scroll regions + Reverse Index. Treat Zellij as a
  separate coco validation target; the current reference source in this
  workspace does not expose a reusable Zellij-specific insertion path.
- `codex-rs/tui/src/transcript_reflow.rs` and
  `codex-rs/tui/src/app/resize_reflow.rs`: treat in-memory transcript cells as
  source of truth, debounce resize work, clear/replay terminal history at the
  new width, and force a second source-backed reflow after streaming output
  consolidates.

Those pieces form one architecture. Porting only `Viewport::Inline` without
custom terminal ownership is the failure mode the previous Phase C hit.

## Core Decisions

1. **Do not use ratatui `Viewport::Inline` directly.**
   `coco-tui` needs explicit ownership of viewport geometry, cursor style,
   history insertion, scrollback clear/replay, and diff invalidation. The
   stock API hides too much state.

2. **Use one native terminal abstraction.**
   Port a `SurfaceTerminal` derived from `codex-rs/tui/src/custom_terminal.rs`
   and adapt it as the only long-term base surface. Fullscreen alt-screen is
   not retained as a compatibility mode. Alt-screen remains only as a temporary
   overlay surface for views that must not overwrite terminal history.

3. **Terminal scrollback is a projection, never source of truth.**
   `SessionState.messages` remains canonical. Terminal rows are disposable
   render output. Resize, `/clear`, rewind/truncate, session switch, and stream
   consolidation must rebuild from message source, not from terminal contents.

4. **Only finalized history enters native scrollback.**
   Streaming text, running tools, classifier state, pending permission dialogs,
   toasts, autocomplete, command palette, and status bars remain in the
   interactive viewport. When a stream/tool/message finalizes, it is emitted as
   history rows exactly once.

5. **Overlay placement is part of the surface contract.**
   Modal surfaces must never overwrite terminal history. Large read-only or
   navigation surfaces enter alt-screen and restore the inline viewport on
   close. Small decision surfaces that need scrollback context, such as
   permission and question prompts, may stay inside the interactive viewport
   only when they fit and can be made attention-safe. Each overlay class must
   declare its surface placement.

6. **Resize reflow is source-backed and part of the final backend.**
   There is no safe selective "delete only rows emitted by coco" terminal
   operation. In native-scrollback mode, resize repair owns the terminal session
   from startup and may purge/replay the visible scrollback. This is the cost of
   making wrapped history correct after width changes. A user-visible explicit
   redraw command is useful, but automatic debounced repair is still required
   for the final implementation.

## Proposed Modules

```
app/tui/src/surface/
  mod.rs
  terminal.rs
  frame.rs
  history_insert.rs
  history_emitter.rs
  transcript_reflow.rs
  vt100_backend.rs        # tests only, if not centralized elsewhere
```

### Surface Configuration

Do not introduce a long-term `SurfaceMode` switch. `NativeScrollback` is the
target surface, and the old fullscreen renderer has been deleted.

Configuration should live under a TUI/display config block, not
`coco_types::Feature`. These settings tune native scrollback behavior; they do
not select between competing UI implementations.

Draft settings shape:

```json
{
  "tui": {
    "native_scrollback": {
      "max_reflow_rows": 9000,
      "zellij_strategy": "auto"
    }
  }
}
```

No user-facing `--alt-screen` or `--native-scrollback` switch is part of the
final design. Temporary validation flags are allowed only on development
branches and must not survive the final migration.

### Ratatui Feature Policy

`coco-rs` uses crates.io `ratatui 0.30`, not the old `codex-rs` fork. The
workspace enables only the features that materially reduce implementation risk:

| Feature | ROI | Decision | Why |
|---|---:|---|---|
| `scrolling-regions` | P0 | Enable now | Exposes backend scroll-region operations used by native scrollback insertion and test backends. This replaces some raw terminal-control plumbing, but does not replace `SurfaceTerminal`. |
| `unstable-widget-ref` | P1 | Enable now | Supports by-reference composable widgets and matches the useful part of `codex-rs/tui/src/render/renderable.rs` without copying its module boundary. Wrap use behind coco presentation traits. |
| `unstable-rendered-line-info` | P1 | Enable now | Provides `Paragraph::line_count` / `line_width` for deterministic overlay, pager, and cell measurement. Do not use it as transcript source-of-truth. |
| `unstable-backend-writer` | P3 | Defer | `CrosstermBackend<W>` already implements `Write`, and raw terminal writes should go through `SurfaceTerminal` / backend methods. Enable later only if code needs `writer_mut()` specifically. |

`ratatui-core 0.1.0` in Cargo.lock is not ratatui 0.10. It is the split-out
core crate used by `ratatui 0.30.x`; `Viewport` and `Terminal::insert_before`
are re-exported by ratatui 0.30 from that internal package.

These features simplify the substrate; they do not change the architecture.
`SurfaceTerminal`, history emission, cursor policy, source-backed reflow, and
overlay placement still need coco-owned code because stock ratatui does not
expose the full viewport and cursor state required by native scrollback.

Unstable-feature fallback:

- if `unstable-widget-ref` changes or disappears, keep `UiRenderable` and render
  concrete widgets through `Widget for &T` plus explicit trait-object adapters
  owned by coco;
- if `unstable-rendered-line-info` changes or disappears, fall back to
  source-backed measurement using `textwrap`, `unicode-width`, and direct
  `Line`/`Span` traversal; this is more code but preserves architecture;
- `scrolling-regions` is treated as stable-enough for the native substrate in
  ratatui 0.30, but the surface layer must keep raw ANSI/VT100 tests around it
  because terminal behavior still varies by emulator;
- do not add direct ratatui unstable APIs to `AppState`, public coco types, or
  presentation APIs exported outside the adapter module.

### `surface::terminal`

Port and adapt the `codex-rs` custom terminal with attribution. This is not a
byte-for-byte copy: the coco version owns the final surface contract, keeps the
recent cursor/suspend invariants, and exposes only the terminal operations that
the rest of `app/tui` is allowed to use. Keep the important state:

- `viewport_area: Rect`
- `last_known_screen_size: Size`
- `last_known_cursor_pos: Position`
- `visible_history_rows: u16`
- two diff buffers
- cursor style and position on `Frame`
- `clear_visible_screen`
- `clear_scrollback_and_visible_screen_ansi`
- `invalidate_viewport`
- `note_history_rows_inserted`

`Tui` should own this terminal directly:

```rust
pub struct Tui {
    terminal: SurfaceTerminal<CrosstermBackend<Stdout>>,
    pending_history_lines: Vec<Line<'static>>,
    alt_saved_viewport: Option<Rect>,
    suspend_context: SuspendContext,
}
```

The recently-added cursor module can stay as the single policy point, but the
claim should be applied through `surface::Frame` instead of crossterm writes
outside the terminal abstraction. That keeps tests deterministic and prevents
`with_terminal(TestBackend)` from touching real stdout.

Target public shape:

```rust
pub struct SurfaceTerminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    viewport_area: Rect,
    last_known_screen_size: Size,
    last_known_cursor_pos: Position,
    visible_history_rows: u16,
    alt_screen_active: bool,
    hidden_cursor: bool,
}

impl<B> SurfaceTerminal<B>
where
    B: Backend + Write,
{
    pub fn draw_viewport<F>(&mut self, area: Rect, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut SurfaceFrame<'_>) -> io::Result<()>;
    pub fn insert_history_lines<I>(&mut self, lines: I) -> io::Result<()>
    where
        I: IntoIterator<Item = Line<'static>>;
    pub fn set_viewport_area(&mut self, area: Rect);
    pub fn invalidate_viewport(&mut self);
    pub fn note_history_rows_inserted(&mut self, rows: u16);
    pub fn clear_owned_scrollback(&mut self) -> io::Result<()>;
    pub fn enter_alt_screen_overlay(&mut self) -> io::Result<()>;
    pub fn leave_alt_screen_overlay(&mut self) -> io::Result<()>;
    pub fn restore_terminal_modes(&mut self) -> io::Result<()>;
}
```

The spike must prove which pieces of this shape are possible against crates.io
`ratatui 0.30`. In particular, the two-buffer diff path must not rely on private
fields that only exist in the `codex-rs` ratatui fork. If the buffers cannot be
owned cleanly, decide between a coco-owned fork and a narrower native target
before presentation work depends on the surface API.

S1 must answer three sub-questions before broad integration:

1. Can `[Buffer; 2]` diff ownership, invalidation, and viewport-pinned redraw be
   implemented without reaching into ratatui-private fields?
2. Does `crossterm::SynchronizedUpdate` keep native insertion plus retained
   viewport redraw coherent on Terminal.app, iTerm2, tmux, and Zellij or its
   compatibility mode?
3. Can clear-visible-screen and clear-owned-scrollback be implemented with
   scroll regions plus raw VT100 writes without corrupting the next diff?

Decision criteria:

- if the missing API is ratatui `Buffer` or cell visibility access, fork ratatui
  or defer native scrollback rather than hiding fork-only assumptions in coco
  code;
- if the missing API is terminal plumbing around writes, cursor style,
  synchronized update, or scroll-region commands, keep crates.io ratatui and
  implement the plumbing in `SurfaceTerminal`;
- if neither path can pass VT100 insertion, clear/replay, cursor, and resize
  tests by the 6-week stop condition, ship the improved in-app transcript/pager
  UX and leave native scrollback blocked.

Before broad integration, prove the port against crates.io `ratatui 0.30`.
`codex-rs` is on a maintained fork, so any dependency on fork-only internal
state must be found early. The spike should compile the first useful slice of
`custom_terminal.rs` into coco, exercise viewport/diff/cursor state with
`TestBackend`, and record whether a ratatui fork is actually required.

### `surface::history_insert`

Port `codex-rs/tui/src/insert_history.rs`, adapted to coco style:

- writer is always `terminal.backend_mut()`, never direct stdout;
- standard mode uses ratatui backend scroll-region operations when possible,
  plus Reverse Index where the backend API does not model the exact terminal
  action;
- Zellij behavior is implemented only after a proven coco strategy from the
  terminal matrix; do not assume a ported implementation exists in the
  reference tree;
- cursor position is restored to `last_known_cursor_pos`;
- line styling is emitted as ANSI spans;
- URL-heavy lines are not hard-split unless needed;
- inserted physical rows update `visible_history_rows`.

Because ratatui 0.30 exposes `scrolling-regions`, the coco port should be much
smaller than codex's 873-line `insert_history.rs`: backend scroll-region
operations and `Terminal::insert_before` tests cover part of the terminal
plumbing. They do not remove the need for `SurfaceTerminal`, visible-history
accounting, Zellij policy, or source-backed replay.

The insertion API should accept a rendered line source rather than forcing every
caller to materialize an unbounded message in memory:

```rust
pub fn insert_history_lines<B, I>(
    terminal: &mut SurfaceTerminal<B>,
    lines: I,
    mode: InsertHistoryMode,
) -> io::Result<()>
where
    B: Backend + Write,
    I: IntoIterator<Item = Line<'static>>;
```

The first implementation may collect internally to preserve the same terminal
write transaction, but single-message emission is still bounded by the same
message-boundary truncation policy used for replay. Very large finalized
messages should emit the newest bounded suffix into native history and rely on
the source-backed transcript pager for full review.

### `surface::history_emitter`

This is the coco-specific layer. It bridges `SessionState.messages` to
terminal history rows.

Responsibilities:

- track the emitted message prefix by stable `ChatMessage.id`;
- detect append-only growth and emit only new finalized messages;
- detect rewind/truncate/non-prefix changes and request replay;
- skip active streaming state until it consolidates into `ChatMessage`;
- defer emission while an alt-screen overlay is active;
- keep separator policy identical to `ChatWidget::build_lines_owned`;
- respect `is_meta`, `show_system_reminders`, and
  `is_visible_in_transcript_only` through an explicit render context.

Sketch:

```rust
pub struct HistoryEmitter {
    emitted_ids: Vec<String>,
    has_emitted_rows: bool,
    replay_required: bool,
}

pub enum HistoryEmission {
    Append(Vec<Line<'static>>),
    Replay(Vec<Line<'static>>),
    None,
}
```

Do not push history directly from server notification handlers. They should
only mutate `SessionState`. The draw pre-pass owns emission so source/render
ordering is deterministic.

The emitter requires an explicit finalization contract before terminal work can
be considered correct:

- a message is history-eligible only after it has a stable `ChatMessage.id` and
  no longer depends on active streaming/tool state;
- transient stream deltas, running tool progress, permission prompts, toasts,
  autocomplete, and status rows never enter native scrollback;
- slash-command output, system/meta messages, hidden transcript-only messages,
  and tool call/result groups are represented through typed render policy, not
  ad hoc filtering inside terminal code;
- the same message source must feed transcript overlay rendering and native
  history rows, so separator and visibility policy cannot diverge.

### `surface::transcript_reflow`

Implement the final source-backed resize repair state machine, using
`codex-rs/tui/src/transcript_reflow.rs` as a reference:

- first observed width initializes state;
- width changes schedule trailing-debounced rebuild;
- height changes also schedule rebuild because rows above the viewport can be
  exposed/hidden;
- resize during stream marks a required post-consolidation reflow;
- pending reflow is deferred while alt-screen overlay is active.

The rebuild algorithm:

1. Drop `pending_history_lines`.
2. Render retained transcript suffix from `SessionState.messages`.
3. Clear visible/scrollback output owned by native mode.
4. Reset viewport to row 0 with desired interactive height.
5. Queue retained rows through `insert_history_lines`.
6. Mark `HistoryEmitter` emitted prefix as matching source.

`max_reflow_rows` is required as an internal rendered-row budget. Without a cap,
very large sessions can turn terminal resize into an unbounded replay path. The
transcript overlay can still read the full `SessionState.messages` source. If
the cap truncates replayed native history, truncation happens at message
boundaries and the user-facing marker reports omitted messages, not rendered
rows whose count changes with terminal width.

Truncation UX:

```text
... 84 older messages retained in transcript, not replayed
    open transcript pager for full history

<newest replayed history row>
<newest replayed history row>

interactive viewport
```

The marker is emitted into native history during replay only. It is not a
transcript-pager content row: the pager always renders the full source-backed
`SessionState.messages` view, so showing the same native-history marker there
would imply false data loss. Diagnostics may log the omitted rendered-row count,
but any row count must be width-qualified, for example "at width 100".

Alt-screen entry and reflow must not interleave. If the user opens a transcript
pager while reflow is active, the surface either waits for reflow to become idle
or cancels the in-flight reflow and rebuilds from source after the pager closes.
No pending history queue may be flushed while alt-screen entry is partially
complete.

### Zellij Detection And Degradation

Until Zellij has its own validated insertion strategy, native mode must treat it
as a compatibility risk instead of silently corrupting history. Runtime terminal
capability detection should set a `TerminalCompatibility` value before the first
draw.

Detection inputs:

- `ZELLIJ` / `ZELLIJ_SESSION_NAME`;
- terminal probe results if environment detection is inconclusive;
- a user override under the TUI display config only for diagnostics.

Behavior before validation:

```text
detected Zellij: native scrollback replay is in compatibility mode
resize repair may be limited; use transcript pager for full history
```

The warning is a banner in the interactive viewport and a diagnostic status
entry, not a modal. Compatibility mode chooses one explicit degraded behavior:
disable native insertion entirely, or limit replay to operations proven safe in
that terminal. Transcript source and pager access remain complete. Disabling
insertion does not restore the old fullscreen alt-screen base UI. It means
finalized history is source-backed and reviewable through the in-app transcript
pager only; native wheel scroll for finalized history is unavailable in that
environment until a validated insertion strategy lands.

## Render Split

The final native surface uses explicit base-surface and overlay renderers.
`FrameLayout` is a small shared output type for cursor arbitration:

```rust
pub fn render_interactive_viewport(
    frame: &mut SurfaceFrame,
    state: &AppState,
) -> FrameLayout;

pub fn overlay_surface_placement(overlay: Option<&Overlay>)
    -> Option<OverlaySurfacePlacement>;
```

`render_interactive_viewport` draws only:

- optional live stream / running tool tail;
- active banners;
- input box;
- status bar;
- inline composer popups;
- toasts.

It must not redraw finalized chat history because those rows already live in
native scrollback. The user sees finalized history above the viewport through
the terminal, not inside a ratatui paragraph.

There is no right-side tool execution list in the final surface. It duplicates
inline tool status, consumes width exactly when tool output benefits from
horizontal space, and creates a second transient projection that native
scrollback cannot preserve. Tool progress appears either as the live tail in
the interactive viewport or as finalized history rows after consolidation.

## Tui Draw Flow

Native draw should look like this:

```text
TuiEvent::Draw / Resize
  → observe terminal size
  → history_emitter.compute(SessionState)
  → maybe schedule/run transcript_reflow
  → Tui::draw_native(desired_height)
      → synchronized update
      → apply pending suspend resume action
      → update inline viewport height/position
      → flush pending history lines above viewport
      → invalidate diff buffer if raw scroll/newline strategy moved rows
      → render_interactive_viewport
      → apply cursor claim
      → flush backend
```

Use `crossterm::SynchronizedUpdate` where supported around native-mode
viewport update + history flush + draw. It reduces flicker when rows are
inserted above the viewport.

## Overlay Policy

Native scrollback cannot let modal overlays overwrite terminal history.

Overlay classes:

- **Alt-screen overlays:** help, transcript, diff, settings, model picker, task
  detail, memory picker, global search, session browser.
- **Inline decision surfaces:** permission, question, feedback, cost warning
  when they fit within the interactive viewport, should preserve native
  scrollback context, and can be made visible to the user.
- **Inline composer surfaces:** autocomplete popup, command palette, pending
  chord hint, paste indicator.
- **History-emitting events:** completed slash output, finalized user input,
  completed assistant/tool/system messages.

When entering alt-screen:

1. Wait for active reflow to become idle, or cancel it and mark replay required
   after the overlay closes.
2. Flush or defer pending history lines.
3. Save inline `viewport_area`.
4. Enter alt-screen.
5. Set viewport to full terminal.
6. Render fullscreen overlay.

When leaving:

1. Leave alt-screen.
2. Restore saved inline viewport.
3. Queue deferred history lines.
4. Invalidate viewport and draw.

Blocking prompts are not allowed to become invisible because the user has
scrolled native scrollback away from the retained viewport. Permission, sandbox,
question, MCP elicitation, and trust prompts use an attention-safe placement:
inline only when visibility is known, otherwise local/alt-screen focus plus bell
or status banner. This is stricter than `codex-rs/tui` because coco's target
lets the terminal own scrollback navigation.

Visibility cannot be queried from ordinary terminal APIs. Treat it as known only
after a recent app-directed interaction with the retained surface, such as a key
event routed to the composer/local surface, a focus-gained event followed by a
successful draw, or the explicit user action that opened the prompt, within a
short window such as 2 seconds. Otherwise upgrade blocking prompts to the
attention-safe path.

## Clear, Rewind, Session Switch

`/clear`, Ctrl-L, rewind, and session switch are native-scrollback state
transitions, not just `SessionState.messages.clear()`.

Required operations:

- clear pending history queue;
- clear or replay terminal scrollback depending on command;
- reset `HistoryEmitter`;
- reset `TranscriptReflowState`;
- reset `visible_history_rows`;
- reset viewport to row 0 before emitting a fresh header/history suffix.

Rewind/truncate is handled by prefix reconciliation:

- if current message IDs start with `emitted_ids`, append only;
- if current source is a strict prefix or diverges, schedule replay;
- replay clears native history and emits current source.

## Suspend / Resume

The recent `job_control` work remains valid but must become surface-aware:

- native mode leaves raw/bracketed/focus modes but does not enter/leave
  alt-screen for the normal surface;
- if an alt-screen overlay is active when suspended, resume restores that
  overlay surface and then redraws;
- cursor y used for shell prompt should be the bottom of the inline viewport
  when in native mode.

This mirrors `codex-rs/tui/src/tui.rs` where `SuspendContext` receives the
inline area bottom and has access to saved alt-screen viewport state.

## Failure Modes

`SurfaceTerminal` owns terminal modes, so it also owns best-effort cleanup.

Hard invariants:

- `SurfaceTerminal::Drop` calls `restore_terminal_modes`.
- The panic hook calls the same best-effort reset path before printing the panic.
- Reset is idempotent and attempts to reset DECSTBM/scroll region, leave
  alt-screen if active, show the cursor, reset cursor style, and disable
  bracketed/focus modes that coco enabled.
- Suspend/resume and Ctrl-C paths use the same reset primitives instead of
  ad hoc crossterm writes.

This cannot handle `SIGKILL`, process abort before destructors, or terminal
emulator crashes because no Rust cleanup code runs in those cases. The support
story for those cases is a documented manual recovery (`reset`, new shell, or
future `coco doctor terminal-reset`), not a false cleanup guarantee.

## Testing Plan

Unit tests:

- `HistoryEmitter` append, truncate, divergence, clear, stream-skip, overlay
  defer.
- `TranscriptReflowState` debounce, stream-finalization repair, width vs
  height changes.
- `SurfaceTerminal` cursor style/position, diff invalidation, clear behavior.

VT100 integration tests:

- inserting one history line shifts viewport down;
- multiple styled spans preserve colors;
- wide/CJK lines preserve cursor position and row counts;
- Zellij insertion strategy, once chosen, invalidates the diff buffer correctly;
- resize reflow clears/replays from source at a new width;
- overlay enter/leave restores inline viewport.

Testing fixture split:

- `ratatui::backend::TestBackend` is for widget snapshots, cell layout, and
  buffer diff tests. It does not emit ANSI bytes.
- Terminal-control tests use a byte-capturing backend parsed by `vt100`, modeled
  after `codex-rs/tui/tests/test_backend.rs` and its `VT100Backend` re-export.
- DECSTBM, Reverse Index, cursor restore, alt-screen enter/leave, and native
  history insertion must be asserted through the VT100 path, not TestBackend.

Live harness / manual matrix:

- macOS Terminal.app
- iTerm2
- tmux
- Zellij
- Linux terminal
- SSH session
- Ctrl+Z / `fg`
- focus regain cursor pin
- long tool output
- `/clear`, rewind/truncate, session switch
- macOS VoiceOver / NVDA through tmux or SSH, verifying finalized history is
  announced as ordinary terminal output.

Observability:

- counter `tui_history_replay_count`;
- counter `tui_history_replay_truncated_count`;
- counter `tui_history_replay_failed_count`;
- counter `tui_stream_consolidation_repair_count`;
- counter `tui_effect_cancelled_count`;
- gauge/counter `tui_zellij_compat_mode_active`;
- span `tui.draw` with width, height, replay reason, emitted row count, and
  overlay state fields, following `common/otel/CLAUDE.md` naming conventions.

## Implementation Evidence Gates

These gates are merge and verification boundaries, not product milestones. Do
not expose native scrollback as the default, or describe it as complete, until
all gates pass. Avoid temporary code paths that would be deleted by a later
gate; build the final abstractions from the first patch.

### Gate S1: Final Surface Substrate

- Port `custom_terminal` into `surface::terminal` with attribution.
- Add VT100 test backend dependency for `coco-tui` tests.
- Record the decision between crates.io ratatui, a coco-owned ratatui fork, raw
  crossterm with coco buffer diff, or deferred native scrollback using the S1
  decision criteria above.
- Move cursor claim application into `surface::Frame`.
- Add `SurfaceTerminal::Drop` and panic-hook cleanup for DECSTBM, alt-screen,
  cursor visibility/style, bracketed paste, and focus mode.
- Preserve the landed hardening invariants: widgets do not set the cursor,
  modal overlays hide or own cursor claims, and suspend errors do not leave an
  unknown terminal state.

### Gate S2: History Source Contract

- Add typed transcript/history render cells over `SessionState.messages`.
- Define finalization, visibility, separator, tool grouping, and meta-message
  policy once.
- Reuse this source contract for transcript overlay rendering and native
  history rows.
- Add append/truncate/divergence/stream-skip tests before terminal insertion
  is wired to real draws.

### Gate S3: History Insertion

- Port `history_insert`.
- Add `Tui::pending_history_lines`.
- Add `HistoryEmitter` over `SessionState.messages`.
- Wire native scrollback as the only base surface in the branch. Do not expose
  a user-facing mode switch.
- Emit startup header and finalized message rows through the same source
  contract.
- Stop rendering finalized history in the native interactive viewport.
- Keep live streaming/running tools in the viewport.

### Gate S4: Overlay Surface Guard

- Add alt-screen guard for large overlays.
- Add inline decision surfaces for permission/question/cost/feedback prompts
  that need scrollback context and can be made attention-safe.
- Defer history rows while overlays are active.
- Prevent alt-screen entry from interleaving with active reflow.
- Restore inline viewport on close.
- Cover help, transcript, diff, model picker, permission, question, and session
  browser paths before considering the surface complete.

### Gate S5: Source-Backed Resize Reflow

- Add `TranscriptReflowState`.
- Add row cap config.
- Clear/replay from `SessionState.messages`.
- Truncate replay at message boundaries and report omitted messages in the
  native-history marker.
- Force post-stream consolidation reflow.
- Add VT100 resize tests and live harness checks.
- Add an explicit redraw/replay command for manual repair and diagnostics.

### Gate S6: Finalization

- Ship native scrollback only after the full macOS/iTerm2/tmux/Zellij/manual
  matrix passes.
- Remove the legacy fullscreen base renderer and any surface switch/config that
  exists only for migration.
- Delete any temporary validation switches or duplicate render paths that are
  no longer part of the final surface contract.

## Rejected Designs

- **Stock ratatui `Viewport::Inline` only.** Too little ownership over cursor,
  diff buffers, history insertion, and resize repair.
- **Render all history both in scrollback and viewport.** Duplicates messages
  and makes scroll position incoherent.
- **Emit streaming deltas directly into scrollback.** Streaming rows are not
  source-backed until consolidation; width changes would preserve transient
  wrapping.
- **Use terminal contents as replay source.** Terminal scrollback is not a
  structured model and cannot safely reconstruct styled message boundaries.
- **Selective scrollback repair.** Terminals do not provide a reliable "delete
  only my historical rows" primitive. Source-backed replay must own the session
  surface while native mode is active.
- **Retain fullscreen alt-screen as a compatibility base surface.** It keeps two
  UI implementations alive and weakens the native scrollback architecture.
- **Right-side tool execution rail.** It duplicates inline tool state, narrows
  the transcript, and adds a transient projection that cannot survive native
  scrollback replay.

## Success Criteria

- Finalized chat history scrolls with terminal-native wheel/trackpad.
- Composer/status/live stream remain stable at the bottom.
- Native selection/copy works for old finalized messages.
- Focus regain cursor always lands in the composer or hides under modal
  overlays.
- Ctrl+Z / `fg` restores terminal modes and redraws correctly.
- `/clear`, rewind, session switch, and resize produce source-backed history
  with no duplicate rows.
- Zellij has a tested insertion strategy.
- The legacy fullscreen base renderer and right-side tool execution rail are
  removed from the final UI.
