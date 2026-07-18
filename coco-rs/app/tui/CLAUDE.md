# coco-tui

Terminal UI using ratatui with The Elm Architecture (TEA). Consumes
`CoreEvent`; emits `UserCommand` back to the host. Events arrive via the
local AppServer bridge's passive event pump (see `app/agent-host`
`local_bridge`), not direct coco-query channel wiring.

## Architecture (TEA)

```
Model (AppState) ← Update (handle_event) ← Events (TuiEvent) ← View (render)
                                              ↑
                                         CoreEvent (AppServer bridge pump)
                                         UserCommand (TUI → host)
```

`App::run` is a `tokio::select!` loop multiplexing: terminal input
(crossterm), `CoreEvent` from the bridge, file-search/symbol-search results,
animation ticks.

## Key Types

| Type | Purpose |
|------|---------|
| `App`, `create_channels` | Run loop + channel constructor |
| `app_events::*` | Event-loop conversion, coalescing, deferred-event, completion, theme/plugin helpers |
| `AppState` | Model: `SessionState` + `UiState` + `RunningState` |
| `state::SessionState` | Messages, tool executions, subagents, token usage, plan_mode, permission_mode, MCP servers, team |
| `state::UiState` | Input, scroll, streaming, suggestions, theme; `modal: Option<ModalState>` + `modal_queue: ModalQueue` |
| `state::ModalState` | Modal enum (`state/modal.rs`, see it for variants); per-variant `priority()` drives the `ModalQueue` |
| `TuiEvent`, `TuiCommand` | Message enum driving update; chord-aware command dispatch |
| `UserCommand` | TUI → host: SubmitInput, Interrupt, ApprovalResponse, ModelChange, etc. |
| `server_notification_handler::handle_core_event` | Fold `CoreEvent` into `AppState` |
| `render` | Pure `(&AppState, &mut Frame) → ()` — ratatui view |
| `Theme`, `ThemeName` | Owned by the `coco-tui-ui` crate, re-exported here; built-in themes plus custom `~/.coco/theme.json` palettes; hot-reloaded |
| `composer::{ComposerSnapshot, AttachmentStore, ResolvedInput}` | App-owned atomic composer state, attachment payloads, queue/history/editor restoration |

See `docs/internal/crate-coco-tui.md` for widget taxonomy, modal catalog, and
snapshot-testing conventions (`insta`).

## Transcript Reader

`Ctrl+O` opens the transcript modal as a cell-level reader: lightweight
`TranscriptCell` metadata for the full message list; the renderer locates
visible cells from `TranscriptState.scroll` and renders only those.
Expansion is selected-cell UI state only: `Tab`/`Shift+Tab` select,
`Enter` expands/collapses, expanded cells capped by a fixed per-cell line
cap. Do not reintroduce a user-facing expansion budget, `Ctrl+E` show-all
mode, or a full transcript `Vec<Line>`/`String` path for overlay rendering.

## Transcript Pipeline (tui-v2)

`src/transcript/` owns the v2 streaming→scrollback pipeline
(`docs/internal/ui/tui-v2-design.md` §6.4): `cells` (`RenderedCell` /
`CellKind` / `SystemCellKind`, engine-message grouping, tool-commit
boundary), `derive` (`Message` → cells), `render/` (the ONLY renderer home:
`cells_renderer.rs` — `CellsRenderer`, the `&[RenderedCell]` → `Vec<Line>`
projection — plus per-category cell renderers, the committed-history
renderer, and the replay cache), `stream` (stable/tail splitter, render key,
watermark; incrementally scans appended source and renders only newly stable
slices, with an authoritative full-prefix fallback for document-global
reference definitions), `emission` (exactly-once tracker + anchored finalize).
The
finalize anchors streamed scrollback rows at the SOURCE level
(`text.starts_with(source_prefix)` + render-key gate) and appends only the
committed render's suffix — no rasterized per-row reconciliation; soundness
pinned by
`transcript::stream::tests::test_stable_lines_are_row_prefix_of_full_committed_render`.
`src/surface/` keeps per-frame drivers and terminal I/O; `src/widgets/` is
frame composition only and depends on `transcript::render`, never the
reverse. Do not reintroduce per-row fingerprints on the stream path, a
second streaming-only renderer, or a renderer under `widgets/`.

**Single scrollback-commit owner (§6.7-10).** The fact "these stream rows are
already in native scrollback" lives in exactly ONE place —
`ScrollbackStreamCommit`, owned by `SurfaceStreamDriver` (`surface/stream.rs`).
The live-tail increment and the anchored finalize both read it
(`SurfaceStreamDriver::commit`); the finalize never keeps its own copy. It is
advanced only by a committed insert (`mark_stream_append_committed`) and cleared
only when those rows actually leave scrollback — `invalidate_commit` (replay /
reset clears scrollback) or `consume_commit` (the finalize folded them into the
message). A transient `streaming == None` frame must NOT clear it (that benign
clear re-committed already-present rows → duplication), and a replay must
invalidate it BEFORE re-preparing the live tail (else the wiped leading rows are
never re-emitted → loss). Do NOT reintroduce a second copy of this state on the
history driver.

## Transcript Invariants

The unified transcript refactor
(`docs/internal/engine-tui-unified-transcript-plan.md`) pins three rules:

- **I-1 Authority** — `coco_messages::MessageHistory` is the single source
  of truth. Every transcript mutation emits one of:
  `MessageAppended` / `MessageTruncated` / `SessionResetForResume`.
  Helpers: `coco_query::history_sync::{history_push_and_emit,
  history_clear_and_emit, history_clear_and_emit_session_reset,
  history_replace_and_emit}`. Direct `history.clear()` / `history.messages = ...`
  in production code is a bug — observers desync.
- **I-2 Derived view** — `TranscriptView.cells` is a pure derivation
  from `&Message` via `transcript::derive::message_to_cells`. Renderers read
  cells; never mutate cells in place.
- **I-3 UI-only state stays UI-only** — `ui.streaming`,
  `session.tool_executions`, modals, toasts. Not part of transcript.

## Modal Pane Architecture

Full-screen modal behavior lives in `src/modal_pane/`; bottom-pane prompt
behavior in `src/bottom_pane/`. `update/interaction.rs` is only the
precedence shell: prompt-first for approve/deny/filter/nav, modal-first for
confirm, autocomplete handled before prompt/modal routing. Modal-specific
key maps live with the modal behavior (`model_picker`, `team_roster`,
`settings`, `permissions_editor`). Keep the `/permissions` editor as its own
modal-pane module (list, add-form, delete-confirm modes) — do not flatten it
into generic picker behavior. The skills, agents, and plugin dialog
interceptors remain in `update/` until their surfaces are migrated.

### Reasoning metadata (side-cache pattern, no I-2 exception)

The engine emits `ServerNotification::ReasoningMetadataAttached
{ message_uuid, duration_ms, reasoning_tokens }` right after `TurnCompleted`
whenever the model reported non-zero reasoning tokens. The TUI handler
stamps `SessionState.reasoning_metadata` keyed by `message_uuid` (`O(1)`, no
cell-walk). Renderers read `Thinking · <duration> · <tokens>` from the
side-cache; the `RenderedCell` itself remains a pure function of `&Message`
(I-2 preserved). The cache is pruned on `MessageTruncated` /
`SessionResetForResume` so it cannot outlive its anchor.

## Ratatui Style Conventions

Apply across all tui crates (`app/tui`, `tui-ui`, `tui-markdown`, `tui-mermaid`):

- Stylize helpers: `"text".dim().bold().cyan()` — avoid manual `Style`
- Simple conversions: `"text".into()`, `vec![...].into()`
- Runtime-computed style: `Span::styled` or `Span::from(t).set_style(s)` is OK
- Avoid `.white()`; prefer default foreground
- Don't refactor between equivalent forms without a readability gain
- Prefer forms that stay on one line after rustfmt
- Text wrapping: `textwrap::wrap` with `initial_indent`/`subsequent_indent` — don't roll your own
