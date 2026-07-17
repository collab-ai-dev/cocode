# coco-tui-ui

Pure, domain-free presentational primitives for the coco TUI. The seam is
**"view-models in, ratatui out"**: this crate holds no `AppState`, no i18n, and
no application dependencies. The `coco-tui` shell projects `AppState` into
`Line`s / plain view models and drives these primitives.

## Dependencies

`ratatui`, `crossterm`, `unicode-width`, `unicode-segmentation`, `tracing`
(logging facade), `base64`, and platform-gated `arboard` (clipboard) / `libc`
(unix). **Must NOT depend on** `coco-config`, `coco-messages`, `coco-types`,
`coco-agent-host`, `coco-query`, `coco-context`, `coco-keybindings`, or `rust-i18n`.
Enforced by `scripts/check-tui-ui-seam.sh` (wired into `just check-seam` →
`quick-check` / `pre-commit`).

## Modules

| Module | Purpose |
|--------|---------|
| `engine` | Native-scrollback paint engine: `SurfaceTerminal<B: SurfaceBackend>`, BSU/ESU synchronized-update framing, cell-diff drawing, history insert/reflow, terminal-capability detection, and the `CursorClaim` type. Consumes `Vec<Line>` + a `&mut Buffer` + a `CursorClaim` — never `AppState`. `engine::seat` owns the viewport seat/pin decision (tui-v2 §6.3): `SurfaceTerminal::seat_viewport(SeatInputs) -> SeatDecision`; the shell supplies intent and applies the decision, overlay/alt-screen policy stays in the shell. I-V1 invariant: `seats_flush` — the viewport always seats flush on finalized history, no pinned-gap exemption. Full grow/shrink/deferral semantics: module docs in `src/engine/seat.rs`. |
| `widgets` | Pure ratatui widgets: `textarea`, `notification`, `spinner_verbs`, `status_indicator`, `diff_display`. (Markdown rendering moved to the sibling `coco-tui-markdown` crate — pulldown-cmark + syntect; mermaid to `coco-tui-mermaid`.) |
| `theme` | `Theme` palette struct + `ThemeName` + 9 built-ins (config-free; the shell owns the `settings.json` / `<config_home>/theme.json` loader + file-watcher). |
| `style` | `UiStyles<'a>` — semantic style accessors over `&Theme`. |
| `color` | RGB→xterm-256 downsampling + terminal color-capability detection (absorbed from jcode). |
| `display` | `SyntaxHighlighting` toggle (config-free; the loader stays in the shell). |
| `diff` | `DiffLineView` / `diff_line_views` row model for `diff_display`. |
| `truncate` | Display-width-aware (CJK/emoji-safe) truncation (absorbed from jcode). |
| `clock`, `frame_rate_limiter`, `double_press`, `constants` | Timing / UI primitives. |
| `clipboard` / `clipboard_copy` / `paste` | Clipboard image capture (async subprocess, `io::Result`), text copy (arboard / OSC-52 / WSL), and the paste buffer (`PasteManager` / `ImageData` / `ResolvedInput`). |

## Invariants

- **No domain types.** `Message`, `AppState`, settings, and translated strings are
  projected by the shell into plain values before reaching this crate.
- **The engine is a passive primitive.** The shell owns the render loop
  (`FrameRequester`), the single cursor decision point
  (`compute_cursor(&AppState)` — this crate only holds the `CursorClaim` *type*
  and applies it after painting), suspend/resume triggers, and the
  `MessageHistory` source of truth.
- **`testing` feature** exposes a few `#[cfg(test)]` helpers (`MockClock`,
  `SurfaceTerminal::visible_history_rows`, `HistoryReflowState::force_due_for_test`)
  to *other* crates' tests, since dependencies are not built under `cfg(test)`.
  `app/tui` enables it in `[dev-dependencies]`.

## Reuse

Consumed by `coco-tui`. The primitives (style/theme/color, truncate, the generic
widgets, the surface engine) are reusable by other TUI surfaces such as
`retrieval/src/tui`; that adoption is future work.
