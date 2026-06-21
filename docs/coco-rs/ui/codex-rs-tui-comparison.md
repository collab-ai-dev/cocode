# Codex-RS TUI UI Design Comparison

Status: reference analysis for `coco-rs` TUI migration.

This document compares the visible UI design of `codex-rs/tui` against the
current and target `coco-rs/app/tui` design. It is intentionally about UI
architecture and user-facing capability. Terminal mechanics are covered in
`native-scrollback-architecture.md`.

## Executive Summary

`codex-rs/tui` is a mature agent-console design. The valuable ideas are:

- committed history and active live content are separate objects;
- input is a bottom-pane system, not just a text box;
- transient local views live near the composer and preserve draft state;
- streaming output has a stable region and a mutable tail;
- large review surfaces use pager-style overlays;
- tool execution, prompts, status, and queue state are first-class visible
  surfaces.

The module ownership is not a good direct fit for `coco-rs`. `codex-rs/tui`
centralizes a large amount of protocol, product, account, config, app-server,
and UI state inside `ChatWidget` and `App`. `coco-rs` should preserve the TEA
boundary:

```text
CoreEvent / terminal input
        -> update
        -> AppState
        -> presentation view models
        -> ratatui widgets
```

The recommendation is to adopt codex's surface contracts and behavior
fixtures, not its object graph.

## Reuse Policy

`codex-rs/tui` is a mature delivered agent console, so coco should reuse its
ideas, terminal mechanisms, interaction details, and tests aggressively. The
boundary is dependency ownership: `coco-tui` must not depend on `codex-tui` as a
crate or import product-specific `codex-*` protocol/config/account types. Reuse
means porting or reimplementing behind coco-owned state, commands, provider
roles, and presentation traits.

| Reuse level | Codex sources | What to reuse | Coco target |
|---|---|---|---|
| Port with adaptation | `custom_terminal.rs`, `insert_history.rs`, VT100 test backend | Native viewport ownership, DECSTBM/Reverse Index insertion, cursor style, diff invalidation, byte-level VT100 assertions. | `surface::SurfaceTerminal`, `surface::history_insert`, `surface::vt100_backend`; verify against crates.io `ratatui 0.30`. |
| Extract behavior and layout | `selection_list.rs`, `pager_overlay.rs`, `diff_model.rs`, `diff_render.rs`, renderable primitives | Dense list behavior, pager progress/footer, row virtualization, diff wrapping/styling, renderable measurement/caching. | `presentation::picker`, `presentation::pager`, `presentation::diff`, `presentation::renderable`. |
| Rebuild on coco state | `bottom_pane/*`, `approval_overlay.rs`, `mcp_server_elicitation.rs`, `request_user_input/*`, `chat_composer/*` | Retained composer state, local surface stack, paste burst semantics, approval/MCP/question routing, Esc/Ctrl-C/Enter focus behavior. | `BottomPaneState`, `SurfaceStackState`, typed prompt view models, `UserCommand` effects. |
| Reuse contract, not variants | `history_cell/*`, `streaming/controller.rs`, `streaming/table_holdback.rs`, exec/status cells | Display/raw/transcript line contracts, stable/tail streaming, table holdback, active-vs-committed split, live-tail consolidation. | Source-backed `TranscriptCell`, `UiRenderable` adapters, `MarkdownStableTail`, `HistoryEmissionController`, `TurnActivityView`. |
| Do not port unless promoted | onboarding, account/status, app link, voice/realtime, product setup flows | Product-specific Codex account/backend UX. | Only add after coco exposes a provider-neutral runtime capability and config path. |

Ported code must carry attribution where derived from codex/ratatui sources and
must replace direct product dependencies with coco equivalents. Any behavior
that touches terminal control, cursor, paste, or prompt focus needs a copied or
rewritten codex-style regression test before it is considered absorbed.

## Source Map

Key `codex-rs/tui` sources:

| Area | Reference files | Useful design |
|---|---|---|
| Top-level app | `app.rs`, `app/*` | Event dispatch, app-server request routing, startup prompts, resize reflow triggers. |
| Main chat surface | `chatwidget.rs`, `chatwidget/*` | Per-session UI controller, active cell, transcript state, turn lifecycle, status and tool lifecycle. |
| Bottom pane | `bottom_pane/mod.rs`, `bottom_pane/bottom_pane_view.rs`, `chat_composer/*` | Composer plus local view stack, paste burst, suggestions, approvals, MCP elicitation, request-user-input. |
| History cells | `history_cell/*` | Typed transcript units with display/raw/transcript lines and measured height. |
| Rendering primitives | `render/renderable.rs`, `render/line_utils.rs` | `Renderable`, `RenderableItem`, flex/column layout, cursor claim, ratatui `WidgetRef`. |
| Streaming | `streaming/controller.rs`, `streaming/table_holdback.rs` | Stable/tail partitioning, table holdback, source-backed resize handling. |
| Pager/review | `pager_overlay.rs`, `diff_model.rs`, `diff_render.rs` | Large read-only overlays, transcript overlay live tail, diff rows. |
| Pickers/status | `selection_list.rs`, `resume_picker.rs`, `theme_picker.rs`, `status/*`, `status_indicator_widget.rs` | Dense list surfaces, tabs, status cards, rate-limit/account details. |
| Terminal shell | `tui.rs`, `custom_terminal.rs`, `insert_history.rs` | Native scrollback surface, cursor style, alt-screen overlays, suspend/resume. |

Current `coco-rs/app/tui` anchors:

| Area | Current files | Current shape |
|---|---|---|
| App loop | `app.rs` | TEA event loop with `CoreEvent`, terminal events, search results, hot reloads, ticks. |
| State | `state/session.rs`, `state/ui.rs`, `state/overlay.rs` | Split `SessionState` / `UiState`; typed overlay enum with priority queue. |
| Rendering | `surface/*`, `overlay_content/*`, `presentation/*`, `widgets/*` | Native history projection plus retained bottom viewport with banners, live tail, input, status/popup, overlays, and toasts. |
| Presentation | `presentation/*` | Emerging view-model layer for pickers, settings, transcript, help, confirmations. |
| Streaming | `state/ui.rs::StreamingState`, `streaming/*` | Simple accumulated text/thinking plus display cursor/adaptive pacing. |
| Terminal | `terminal.rs`, `surface/terminal.rs`, `cursor.rs`, `job_control.rs` | Custom native-scrollback terminal surface with centralized cursor policy. |

## High-Level Design Difference

| Dimension | `codex-rs/tui` | Current `coco-rs/app/tui` | Target for coco |
|---|---|---|---|
| Primary surface | Native-scrollback-style terminal with retained interactive viewport. | Fullscreen alt-screen frame. | Native scrollback as the long-term base, with alt-screen only for large overlays. |
| Conversation model | Committed `HistoryCell`s plus mutable `active_cell`. | `ChatMessage` list plus `StreamingState`; render rebuilds lines directly. | Typed transcript cells derived from `SessionState.messages`; active streaming/tool cell is separate. |
| Input model | `BottomPane` owns retained `ChatComposer` and stack of local views. | `InputState`, paste manager, active suggestions, and central `Overlay`. | Composer stays retained; add a local `SurfaceStack` that integrates with overlay priority. |
| Overlay model | Bottom-pane views for local prompts; pager overlays for large review surfaces. | One typed overlay enum rendered mostly as centered modals. | Placement-aware surfaces: inline prompt, bottom-pane local view, or alt-screen pager. |
| Streaming model | Stable region commits to scrollback; mutable tail remains active; table holdback. | Display cursor over one accumulated string; final content becomes message on turn completion. | Source-backed stable/tail streaming before native scrollback ships. |
| Activity model | Exec cells, active hook cells, unified exec footer, status indicator. | Tool messages, side panels, banners, queue/status widgets. | One `TurnActivityView`; no persistent right rail in final UI. |
| State ownership | Large `ChatWidget` owns much of UI and protocol-derived runtime state. | Central `AppState`, update handlers, pure render path. | Keep TEA; codex behavior becomes view models and reducers, not a new mega-controller. |
| Provider/product coupling | OpenAI/Codex account, app-server, onboarding, rate-limit, product flows embedded. | Multi-provider coco runtime with typed `ModelRole` and config layers. | Port only provider-neutral behavior; product-specific flows require runtime support first. |

## What Coco Should Copy

### 1. History Cell Contract

Codex's `HistoryCell` is the most important UI abstraction. Each cell can
produce:

- rich display lines for the main viewport;
- raw/copy-friendly lines;
- transcript overlay lines;
- measured height at a given width;
- animation/cache invalidation signals when live content changes over time.

`coco-rs` currently renders chat by walking `ChatMessage` values inside
`widgets/chat/mod.rs` and then converting that to line output. This is adequate
for fullscreen rendering but weak for native scrollback because the same
message must be renderable in three contexts: committed terminal history,
active live viewport, and transcript/pager overlay.

Build direction:

- add `presentation::conversation` transcript cell view models;
- derive cells from `SessionState.messages`, `tool_executions`, plans, hooks,
  MCP state, and local command results;
- keep `ChatMessage.id` as the history-emission key;
- use ratatui line measurement for display height, but keep source messages as
  canonical truth.
- keep renderers as enum-dispatched presentation adapters. Do not store
  `Box<dyn UiRenderable>` as transcript source or duplicate long message text
  into every cell.

Do not copy codex's product-specific cell variants directly. Use the cell
contract and rebuild variants against coco event types.

### 2. Active Cell Separate From Committed History

Codex's main viewport is not "all history plus input". It has a committed
history list and a mutable active cell for in-flight work. This is a better
mental model for native scrollback:

- finalized history is written once;
- active streaming/tool output can mutate;
- active hooks or tool groups can update in place;
- transcript overlay can append a cached live tail without treating it as
  committed history.

`coco-rs` should adopt this split before emitting native history. Otherwise
streaming lines will either duplicate into scrollback or be difficult to repair
on resize.

### 3. Bottom Pane As A Local Interaction System

Codex's `BottomPane` is not only an input widget. It owns:

- retained `ChatComposer` draft state;
- a stack of `BottomPaneView` local surfaces;
- local key routing before global interrupt/quit handling;
- paste-burst flushing;
- composer history search;
- pending queued input previews;
- delayed approvals while the user is actively typing;
- prompt and MCP elicitation surfaces that can consume incoming requests.

`coco-rs` already has a strong central overlay priority queue. Keep that. Add a
presentation-level `SurfaceStack` for surfaces that should live near the
composer and preserve input state. The routing order should be:

```text
focused local surface -> keybinding context -> composer -> global command
```

This avoids forcing every small prompt into a centered modal while still
preserving coco's security-priority overlay rules.

Codex can keep blocking approvals in the bottom-pane model because its retained
viewport remains the active app surface. Coco's native-scrollback target adds one
extra rule: a blocking prompt must be attention-safe if the user may have
scrolled terminal history away from the retained viewport.

### 4. Stable/Tail Streaming

Codex's streaming controller partitions rendered markdown into:

- stable lines that can be committed in order;
- a mutable tail that can still change;
- table holdback so pipe tables do not reflow already-committed rows;
- resize repair from the source markdown rather than from displayed rows.

`coco-rs` streaming is currently simpler: accumulated content plus a display
cursor. That is fine for fullscreen rendering, but not enough for native
scrollback. Once history is emitted into terminal scrollback, line order and
wrapping become externally visible.

Build direction:

- introduce a source-backed stream controller before native history emission;
- hold back unterminated lines and table regions;
- commit stable lines through the same history emitter used by finalized cells;
- re-render from source on width changes;
- drop active stream buffers on finalization, and segment very large active
  streams into stable emitted segments plus a mutable tail to keep resize work
  bounded.

### 5. Pager And Diff Surfaces

Codex's `pager_overlay.rs` is a strong design for large read-only content:
content is a list of renderables, height is measured by width, scrolling is
stateful, a bottom bar shows progress, and transcript overlay can include a
live active-cell tail.

`coco-rs` has typed overlays, but many are still rendered as `(title, body,
color)` strings in centered paragraphs. That limits diffs, long plans,
transcripts, search results, and status detail.

Build direction:

- create a shared `presentation::pager`;
- make large read-only overlays render typed rows/renderables, not strings;
- use alt-screen pager placement for content that should not overwrite native
  scrollback;
- keep small decision prompts inline only when they need scrollback context and
  satisfy the native-scrollback attention rule.

### 6. Dense Picker Scaffolding

Codex has strong list UI primitives: selection lists, tabs, column-width modes,
side content, narrow-width snapshots, resume picker, theme picker, keymap
picker, and status setup views.

`coco-rs` already has `presentation::picker` and a provider-neutral model picker.
Extend that direction rather than porting codex list types wholesale.

Build direction:

- keep model rows from `SessionState.model_catalog` and `ModelRole`;
- add shared row metadata: primary label, secondary label, badge, disabled
  reason, group, tab, preview, action;
- support dense/narrow/wide layouts from one scaffold;
- make command, skill, file, session, memory, MCP, settings, and model pickers
  share selection behavior.

### 7. Tool And Activity Presentation

Codex makes tool execution visible through active exec cells, completed history
cells, unified exec footer, status indicator, and hook cells. This is closer to
what long-running agent work needs than a simple spinner.

`coco-rs` currently has several separate widgets: tool messages, plan panel,
subagent panel, task list, hook panel, banners, queue status, and stream stall.
The final design already says there should be no persistent right-side rail.

Build direction:

- add `TurnActivityView` as the single live activity model;
- include running tools, subagents, hooks, plans/todos, background tasks, queue
  state, stream health, and interrupt state;
- make each producer declare `ActivityProducerPriority` before rendering so
  narrow-width collapse behavior is stable and testable;
- commit completed activity to transcript/history cells;
- migrate side-panel-era information into the inline activity area above the
  composer; do not retain a permanent right rail.

## What Coco Should Not Copy

| Codex design | Why not copy directly | Coco alternative |
|---|---|---|
| Giant `ChatWidget` owner | It mixes protocol-derived state, product state, config, UI, command dispatch, and rendering support. | Keep `AppState` plus update handlers; add presentation view models. |
| App-server/account-specific flows | They assume Codex product backends, ChatGPT account state, credits/rate-limit semantics, onboarding, and app links. | Add provider-neutral surfaces only when coco runtime exposes typed state. |
| Old ratatui fork assumptions | Codex uses a maintained fork and custom terminal APIs. | Use crates.io `ratatui 0.30` plus coco-owned `SurfaceTerminal`. |
| Product/decorative surfaces | Pets, product onboarding, voice/realtime, account-specific status are not core coco UI requirements today. | Keep P3 unless promoted by a runtime feature and config path. |
| Direct raw terminal writes in widgets | This breaks testability and surface ownership. | Only the surface/terminal layer performs terminal side effects. |
| Display-only model/provider inference | Codex UI is single-product; coco is multi-provider. | Use structured provider/api/model ids and `ModelRole`. |

## Current Coco Gaps Against Codex UI

| Gap | Evidence in current coco | Impact | Priority |
|---|---|---|---|
| No typed committed/live transcript cell boundary | `widgets/chat/mod.rs` renders directly from messages; `StreamingState` is UI-local text. | Native scrollback cannot safely know what is finalized, active, raw, or replayable. | P0 |
| Streaming is not source-backed stable/tail | `StreamingState` stores `content`, `thinking`, `display_cursor`; no table holdback. | Tables and wrapped markdown can reshape after rows are emitted. | P0 |
| Input and local prompts are split across input, suggestions, and central overlays | `UiState` has `input`, `active_suggestions`, and one `Overlay`. | Small blocking prompts become modal-heavy and can lose bottom-pane ergonomics. | P1 |
| Overlay rendering still string-centric | `overlay_content/mod.rs` mostly maps variants to `(title, body, color)`. | Long diffs/status/transcripts are hard to navigate and test at row level. | P1/P2 |
| Activity is fragmented | Tool messages, banners, task panel, subagent panel, hook panel, status bar. | Users do not get one stable place to understand live work. | P1/P2 |
| Picker scaffolding is partial | `presentation::picker` exists, but many surfaces still produce strings. | More duplicate key/focus/narrow-width logic as new surfaces are added. | P2 |
| Terminal surface now uses native scrollback | `terminal.rs` drives `surface::terminal`; `surface::viewport` owns the retained bottom viewport and cursor policy. | Continue moving large dialogs toward typed pager/picker view models. | P0 complete |
| Attachment and code-block display policy is incomplete | Paste/image support and markdown widgets exist, but final transcript cell policy was not explicit. | Implementers may add image or highlighting paths that bypass source-backed cells. | P2 |

## Recommended Build Dependencies

These are dependency priorities, not product phases. The final delivered console
is the target; the labels below identify what later work depends on.

### P0: Substrate And Transcript

1. Add `SurfaceTerminal` and frame/cursor ownership per
   `native-scrollback-architecture.md`.
2. Add typed transcript cells with display/raw/transcript line contracts.
3. Add a history emitter keyed by stable message ids.
4. Replace simple streaming emission with source-backed stable/tail streaming.
5. Add VT100 coverage for insert, resize, cursor, and replay.

Follow the codex testing pattern here: buffer snapshots use ratatui test
backends, while terminal-control behavior uses a byte-capturing VT100 backend.
`TestBackend` output is not an ANSI stream and should not be parsed by `vt100`.

This is the minimum before native scrollback can be considered real.

### P1: Interaction Core

1. Add `presentation::renderable` using ratatui `WidgetRef` and line-info
   features behind coco traits.
2. Add `SurfaceStack` for bottom-pane local views.
3. Move permission, request-user-input, and MCP elicitation into typed prompt
   view models.
4. Unify slash/file/skill/agent/mention suggestions under one row model.
5. Harden markdown/table rendering with source-backed snapshots.

This creates the core user experience codex has without importing codex's
state ownership.

### P2: Breadth And Polish

1. Add shared pager and diff renderables.
2. Extend picker scaffolding to memory, sessions, settings, MCP, commands, and
   keybindings.
3. Add `TurnActivityView` and migrate side-panel information into it.
4. Add terminal peripherals behind services: clipboard, external editor, title,
   notification.

### P3: Product-Specific Surfaces

Do not port onboarding, account, credits, voice/realtime, or decorative surfaces
unless the coco runtime adds a provider-neutral capability and config.

## Ratatui Feature Boundary

The canonical feature policy lives in
`native-scrollback-architecture.md#ratatui-feature-policy`. The important
comparison finding is narrower: ratatui features help implement the design, but
they do not provide the design themselves.

Stock ratatui still does not expose enough terminal state for coco's target
native scrollback design: viewport geometry, cursor style, visible history row
accounting, diff invalidation, scrollback replay, and overlay placement remain
coco-owned responsibilities.

## Acceptance Criteria For Borrowed UI Behavior

Every borrowed codex behavior should satisfy these checks before landing in
`coco-rs`:

- The source of truth is `AppState` / `SessionState`.
- Rendering consumes typed view models and semantic styles.
- The behavior has a narrow/normal/wide snapshot when layout-sensitive.
- Native-scrollback-sensitive behavior has VT100 or terminal-matrix coverage.
- Provider-specific product assumptions are absent or explicitly gated by a
  provider-neutral runtime capability.
- Cursor, paste, Esc, Ctrl-C, Enter, Tab, and PageUp/PageDown behavior is
  documented for the surface.

## Final Judgment

Most visible `codex-rs/tui` UI capabilities are relevant to `coco-rs`: bottom
pane, local view stack, transcript cells, active cell, source-backed streaming,
prompt surfaces, pager/diff, dense pickers, and live activity are all high-value.

The direct code shape is the part to reject. `coco-rs` should stay TEA-driven,
multi-provider, and presentation-model based. Codex's design should inform
contracts, edge cases, and tests; coco should own the implementation boundaries.
