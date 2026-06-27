# coco-tui Current Implementation Baseline

Status: current-state documentation for `coco-rs/app/tui`.

This file describes what the TUI does today. The final product architecture
lives in `docs/coco-rs/ui/agent-console-design.md`; terminal-surface
constraints live in `docs/coco-rs/ui/terminal-surface-design.md`. If the
documents disagree, treat this file as the source of current facts and
`ui/agent-console-design.md` as the source of intended product direction.

Do not copy source statistics or long enum listings into this document. The TUI
surface changes quickly; use the canonical source files named below for exact
variant lists.

## Sources

Primary current sources:

- `coco-rs/app/tui/CLAUDE.md`
- `coco-rs/app/tui/src/app.rs`
- `coco-rs/app/tui/src/state/session.rs`
- `coco-rs/app/tui/src/state/ui.rs`
- `coco-rs/app/tui/src/state/modal.rs`
- `coco-rs/app/tui/src/state/interaction.rs`
- `coco-rs/app/tui/src/events.rs`
- `coco-rs/app/tui/src/command.rs`
- `coco-rs/app/tui/src/terminal.rs`
- `coco-rs/app/tui/src/surface/`
- `coco-rs/app/tui/src/presentation/`
- `coco-rs/app/tui/src/surface_content/`
- `coco-rs/app/tui/src/widgets/`
- `coco-rs/common/types/src/event.rs`
- `coco-rs/app/cli/src/tui_runner.rs`

Reference-only sources:

- The TS project is a behavior reference for provider-neutral UI flows, not a
  Rust module template. TS file paths in these docs are relative to that
  project's `src/` directory.
- `codex-rs/tui` is a terminal-mechanics reference for ratatui patterns.
- `cocode-rs/` and `codex-rs/` are not the active implementation.

## Role

`coco-tui` is a ratatui/crossterm terminal UI built around The Elm
Architecture:

```text
terminal input / CoreEvent
        -> keybinding and event dispatch
        -> update
        -> AppState
        -> presentation view models
        -> native surface draw
        -> terminal backend
```

The crate owns interactive terminal state, rendering, key routing, autocomplete,
modals, interaction prompts, theme state, and notifications. It does not own the agent runtime,
session persistence, tool execution, model invocation, or slash-command
registry construction. Those side effects are orchestrated by
`app/cli/src/tui_runner.rs` and lower layers.

## Dependencies

`coco-tui` uses:

- `coco-types` for `CoreEvent`, protocol payloads, permission modes, model
  roles, reasoning efforts, and common event data.
- `coco-config` for typed environment helpers and display/theme integration
  points.
- `coco-context`, `coco-file-search`, `coco-file-watch`, `coco-git`, and
  `coco-keybindings` for TUI-local services.
- `ratatui`, `crossterm`, `tokio`, `tokio-stream`, `rust-i18n`, `nucleo`,
  `tracing`, `uuid`, and clipboard support.

The crate should not grow direct dependencies on inference, tool execution, or
session-runtime internals just to make a widget easier to render. Runtime
effects should cross the `UserCommand` / `CoreEvent` boundary.

## Event Loop

`App::run` in `app.rs` is the main loop. It multiplexes:

- crossterm keyboard, resize, focus, and paste events
- `CoreEvent` notifications from the agent driver
- async file-search results
- async symbol-search results
- keybinding, theme, display-setting, and config-reload warnings
- tick and spinner timers

High-volume core events are coalesced before redraw so streaming token deltas
do not force one terminal draw per delta.

`create_channels()` returns:

- `mpsc::Sender<UserCommand>` and `Receiver<UserCommand>` for TUI -> runner
- `mpsc::Sender<CoreEvent>` and `Receiver<CoreEvent>` for runner -> TUI

## CoreEvent Consumption

`CoreEvent` has three layers in `common/types/src/event.rs`:

- `CoreEvent::Protocol(ServerNotification)` for SDK-visible session, turn,
  model, task, MCP, hook, sandbox, queue, rewind, cost, and system events.
- `CoreEvent::Stream(AgentStreamEvent)` for streaming text, thinking, tool,
  and MCP tool-call state.
- `CoreEvent::Tui(TuiOnlyEvent)` for TUI-exclusive modals/prompts, toasts, local
  command results, and UI-only prompts.

The TUI handles all three layers in `server_notification_handler/`:

- `protocol.rs` folds `ServerNotification` into `SessionState`.
- `stream.rs` folds `AgentStreamEvent` into streaming/tool display state.
- `tui_only.rs` opens modals/prompts, records local command output, and surfaces
  UI-only status.

The TUI does not define a separate protocol. It consumes the shared event
types and emits `UserCommand` responses.

## State Model

`AppState` is the model:

- `SessionState`: agent-synchronized facts.
- `UiState`: local terminal state.
- `RunningState`: application lifetime state.

### SessionState

Canonical source: `app/tui/src/state/session.rs`.

Important current fields:

- `messages: Vec<ChatMessage>`
- `model`, `provider`
- `model_catalog: Vec<ModelCatalogEntry>`
- `provider_statuses: HashMap<String, ProviderStatus>`
- `model_by_role: HashMap<ModelRole, ModelBinding>`
- `permission_mode: PermissionMode`
- `bypass_permissions_available`, `auto_mode_available`
- `tool_executions`, `subagents`, `active_tasks`, `plan_tasks`,
  `todos_by_agent`, `active_hooks`
- `token_usage`, `context_window_used`, `context_window_total`
- `mcp_servers`, `lsp_active`
- `queued_commands`
- `available_commands`, `available_agents`, `saved_sessions`
- `thinking_effort`, `fast_mode`, model fallback/rate-limit/stream-health
  banners

Plan mode is not a separate session boolean. It is derived from
`permission_mode == PermissionMode::Plan`.

Model data is intentionally structured. Production paths seed
`model_catalog`, `provider_statuses`, and `model_by_role` from
`RuntimeConfig` in `app/cli/src/tui_runner.rs` before the TUI opens the model
picker.

### UiState

Canonical source: `app/tui/src/state/ui.rs`.

Important current fields:

- `input: InputState`
- `paste_manager`
- `scroll_offset`, `focus`
- `modal: Option<ModalState>`
- `modal_queue: ModalQueue`
- `streaming: Option<StreamingState>`
- `active_suggestions: Option<ActiveSuggestions>`
- `theme`, `theme_state`, `display_settings`
- `toasts`
- `collapsed_tools`
- `help_scroll`
- exit and rewind double-press trackers
- `terminal_focused`
- `clipboard_lease`
- `kb_handle`
- input stash and teammate preview flags

Autocomplete is unified under `active_suggestions`. There are not separate
long-lived state fields for file, skill, agent, and symbol popups.

### InputState

`InputState` stores text, cursor position, and frecency-ranked history.
Cursor positions are character indexes, not byte offsets.

Current prefix modes:

- normal chat input
- `!` bash mode

## Modal and Interaction Surfaces

Canonical sources: `app/tui/src/state/modal.rs` and
`app/tui/src/state/interaction.rs`.

`ModalState` is the typed full-screen/modal state for model and session
pickers, search, quick open, export, diff view, worktree/trust confirmation,
task detail, feedback, MCP server select, context visualization, rewind,
settings, memory dialog, transcript view, help, and errors.
`InteractionState` owns bottom-pane prompt-class surfaces such as permission,
question, elicitation, sandbox, cost warning, plan entry/exit/approval, and MCP
approval prompts.

`UiState::show_modal` applies priority ordering:

| Tier | Current variants | Meaning |
|---|---|---|
| 3 | `BypassPermissions`, `WorktreeExit` | high-stakes confirmation |
| 4 | `Error`, `InvalidConfig` | error surface |
| 5 | `Rewind`, `DiffView` | content review |
| 6 | `AutoModeOptIn`, `Trust`, `Bridge`, `McpServerSelect` | settings or trust confirmation |
| 7 | model/session/search/open/export/feedback/task/doctor/context/settings/transcript/memory/idle-return user surfaces | user-triggered surfaces |
| 8 | `Help` | read-only reference |

Lower number wins. A higher-priority modal displaces the active modal into
the queue. Same-or-lower priority modals are inserted into
`modal_queue` in priority order.

## User Commands

Canonical source: `app/tui/src/command.rs`.

`UserCommand` is the outbound TUI -> runner contract. It covers user input,
local bash submission, permission responses, permission-mode changes,
thinking-effort changes, role-aware model selection, slash/skill execution,
queueing, compaction, rewind, clear, plan approval, idle hooks, and shutdown.

The current long-term model-switching entrypoint is:

```rust
UserCommand::SetModelRole {
    role: ModelRole,
    provider: String,
    model_id: String,
    effort: Option<ReasoningEffort>,
}
```

Model changes should keep this structured provider/model/role/effort shape.
Do not add display-only model strings that bypass `ModelRole`.

## TUI Commands

Canonical source: `app/tui/src/events.rs`.

`TuiCommand` is the high-level command produced by keybinding resolution. It
covers mode toggles, input editing, cursor movement, scrolling, focus movement,
modal/prompt approval/navigation/filtering, model-picker role/effort axes, task
management, editor/clipboard operations, display toggles, transcript toggles,
stash, expanded views, and exit handling.

Do not duplicate the enum in docs. Keep source as the exact catalog.

## Terminal Input and Mouse

`terminal.rs` enables raw mode, alternate screen, bracketed paste, and focus
change events. It deliberately does not enable mouse capture.

Current behavior:

- `TuiEvent` has key, resize, focus, tick, spinner, paste, and classifier
  events.
- There is no mouse event in `TuiEvent`.
- `app.rs` drops any stray `Event::Mouse` defensively.

This preserves native terminal drag-to-select and Cmd/Ctrl-C behavior.

## Keybinding Routing

Keybinding code lives in:

- `keybinding_setup.rs`
- `keybinding_resolver.rs`
- `keybinding_bridge.rs`
- `keybinding_dispatch.rs`

The bridge derives an active context from `AppState`, then maps crossterm keys
through the configured keybinding handle. The context order is modal and
interaction-surface state first, autocomplete next, then global/input commands.

Keybinding configuration is hot-reloaded by the CLI runner and surfaced as
toasts through the TUI event loop.

## Transcript Reader

`Ctrl+O` / `app:toggleTranscript` opens the transcript modal as a cell-level
reader. The projection keeps lightweight `TranscriptCell` metadata for the full
message list, and the modal renderer uses the viewport height plus
`TranscriptState.scroll` to locate and render only visible cells.

Expansion is selected-cell UI state only. `Tab` / `Shift+Tab` moves among
expandable cells, and `Enter` expands or collapses the selected cell. Expansion
state is not persisted into the session transcript.

There is no user-facing transcript expansion budget and no `Ctrl+E` show-all
mode. Large expanded cells use an internal fixed line cap and show a truncation
hint in the UI.

## Model Picker Baseline

Current model picker state lives in `ModelPickerState`:

- `role: ModelRole`
- provider/model `entries`
- filter text
- selected filtered-entry index
- optional selected effort

Current behavior:

- Tab and Shift-Tab cycle role across Main, Fast, Plan, Explore, Review,
  HookAgent, Memory, and Subagent.
- Subagent is a real `ModelRole`: it is the default LLM binding for subagent
  execution when no more specific subagent role is selected.
- Up/Down changes model selection.
- Left/Right changes thinking effort.
- Confirm emits `UserCommand::SetModelRole`.
- Model and thinking-effort changes are reflected in the header/status bar, not
  duplicated as success toasts.
- Rows are grouped by provider display name and carry typed unavailable
  reasons.
- Production rows come from `SessionState.model_catalog` and
  `SessionState.provider_statuses`.
- An empty `SessionState.model_catalog` yields no model rows. Tests and
  pre-bootstrap mocks seed catalog entries explicitly; production does not
  infer provider from model-id prefixes.

Target product changes are documented in `ui/agent-console-design.md`;
terminal-surface constraints are documented in
`ui/terminal-surface-design.md`.

## Memory Baseline

Current memory UX has one entrypoint:

- `/memory` opens `ModalState::MemoryDialog` from
  `TuiOnlyEvent::OpenMemoryDialog`.
- `!` is the only input-mode prefix. Leading `#` is ordinary chat text; memory
  editing is routed through `/memory`.

The `/memory` dialog receives typed rows:

- managed
- user
- project
- project-local
- subdirectory
- row kind (`file`, `folder`, or `toggle`), currently produced as file rows

The dialog opens file rows through the CLI editor/opener boundary. Non-file
rows are rendered with their kind tag and are not treated as editor targets.

## Autocomplete Baseline

Canonical sources:

- `app/tui/src/autocomplete/`
- `app/tui/src/widgets/suggestion_popup.rs`

Current triggers:

| Trigger | Kind | Data source | Mode |
|---|---|---|---|
| leading `/` | slash command | `session.available_commands` | sync |
| `@agent-` | agent | `session.available_agents` | sync |
| `@path` | file | `FileSearchManager` | async |
| `@#symbol` | symbol | `SymbolSearchManager` | async |

Async triggers install a pending `ActiveSuggestions` value, then `App` dispatches
file or symbol search when the `(kind, query)` pair changes. Stale async results
are discarded if the active trigger no longer matches.

The autocomplete keybinding context activates only when suggestion items are
available, so empty async searches do not hijack history navigation.

## Rendering Baseline

Top-level drawing is owned by `terminal::Tui` and `surface::controller`.
Finalized history is emitted through `surface::history_driver`; the retained
bottom viewport is rendered by `surface::viewport`.

- header/history projection
- lifecycle and status banners
- live conversation tail
- inline live activity surface
- input area
- queued command list
- status bar
- modals and interaction prompts
- toasts

Message rendering is split under `widgets/chat/`:

- `render_user.rs`
- `render_assistant.rs`
- `render_tool.rs`
- `render_system.rs`

Text modal content is split under `surface_content/` and presentation modules.
Many modals still produce `(title, body, color)` and are placed by
`surface::modal`; large review/navigation modals use alt-screen placement
and restore the retained viewport on close.

## Layout Baseline

The top-level layout is a full-screen ratatui frame:

```text
header
main conversation
inline live activity surface
input and queued-command area
status bar
modals, interaction prompts, and toasts above the base frame
```

The automatic right-side tool execution list was removed to keep the transcript
as the primary surface and avoid a second transient projection of tool state.
Task, teammate, coordinator, stream, and tool status now enter the same inline
activity view above the composer. Coordinator panel selection is resolved into
`UiState` by the CLI runner before rendering.

## Theme and Display Settings

TUI theme state is separate from `RuntimeConfig`:

- `~/.coco/theme.json` is loaded and watched by `theme::install_theme`.
- valid reloads update `UiState.theme` and `UiState.theme_state`.
- invalid reloads surface warning toasts and keep the prior palette.

Display settings come from `settings.json` via the CLI runner and hot-reload
into `UiState.display_settings`; TUI behavior belongs in the existing settings
pipeline rather than a separate TUI-specific config file.

`UiStyles` is the semantic facade over the active `Theme`. It is used by
modal frame/content helpers, composer chrome, footer/toast/activity,
lifecycle banners, stash/queue/suggestion widgets, teammate header, and the
request/confirm/model/picker/settings presentation surfaces. Rich transcript /
chat, markdown/diff rendering, and specialist panels consume the same facade;
the top-level renderer constructs `UiStyles` once from `UiState.theme` and
passes the facade through concrete renderers.

## Notifications and Clipboard

Terminal notification delivery lives in `widgets/notification.rs`.
It detects iTerm2, Kitty, Ghostty, terminal bell, tmux, and screen cases and is
used for turn-complete notifications when the terminal is unfocused.

Clipboard copy lives in `clipboard_copy.rs`:

- SSH prefers OSC 52.
- local sessions prefer `arboard`.
- Linux may keep a clipboard lease in `UiState`.
- macOS suppresses noisy pasteboard stderr while initializing.
- WSL PowerShell is a fallback.
- OSC 52 has a raw payload cap to avoid terminal abuse.

Copy-last-agent-response is available through `/copy` and the configured
clipboard keybinding.

## Known Deviations, Conflict Register, and Migration Debt

These are current facts, not target design:

| Area | Current state | Target direction |
|---|---|---|
| Memory prompt mode | Removed. `#` is ordinary chat text; TS `components/PromptInput/inputModes.ts` recognizes only `!` as an input mode character. | Keep `/memory` as the only memory entrypoint. |
| Direct memory append | Removed. The input bar no longer writes directly to `CLAUDE.md`. | Keep memory editing routed through the `/memory` picker and editor/opener service. |
| Memory dialog payload | `OpenMemoryDialog` carries path/label/scope plus row-kind-aware file/folder/toggle semantics. Current producer emits file rows. | Keep renderer and selection behavior keyed by row kind. |
| Input rendering | `widgets/input.rs` owns the input view model and renderer; `surface::viewport` only wires state into the widget. | Keep composer presentation centralized. |
| Render-time config/env decisions | Coordinator mode is resolved into `UiState` before draw; surface renderers do not read config/env helpers. | Keep display decisions in state/view models. |
| Theme coupling | Modal, chrome, input, transcript/chat, markdown/diff, and specialist widget renderers use `UiStyles`. Direct `Theme` access is limited to theme loading/state and the top-level `UiStyles::new(&state.ui.theme)` adapter, plus tests that create default themes. | Keep new and changed surfaces on `UiStyles`; add semantic accessors instead of passing `Theme` into renderers. |
| Modal rendering | Many modals are still string-body dialogs. | Complex modals move to typed view models and shared dialog/picker/pager primitives. |
| Model picker fallback | Removed. Empty catalogs no longer synthesize builtin rows or infer providers from model ids. | Keep production model rows catalog-backed; tests/pre-bootstrap mocks seed catalog entries explicitly. |
| Editor/open effects | `/memory`, prompt external-editor, and plan-editor requests route through the CLI command boundary. The runner resolves the concrete session plan file from runtime config, asks the TUI App to leave raw mode / alt-screen before launching an editor, and the TUI restores modes before applying completion events. | Keep editor/open effects in the runner; future opener flows should reuse the same terminal handoff. |
| Conversation/diff rendering | Structured diff row parsing, source-backed transcript presentation cells, streaming-tail blocks, same-hook lifecycle batching, task-notification/background-bash/teammate-shutdown batching, active streaming/busy-tail cells, and pager windows live in presentation view models before widgets render ratatui lines. | Keep future conversation surfaces and display-collapse reducers on these view models before adding widget logic. |
| Activity surface | `presentation::activity::TurnActivityView` resolves whether the main area uses no activity, a plan surface, an agent/coordinator surface, or a general activity surface. `widgets::ActivityPanel` renders the unified inline row view model directly above the composer. | Keep live plan/tool/agent/activity rows behind the presentation view model before ratatui rendering. |
| Input/suggestions/footer | `widgets/input.rs` owns the composer render model; `presentation::input::InlinePopupView` resolves autocomplete and command-palette popup rows; `presentation::footer::FooterView` resolves status-bar and exit-prompt props. | Keep suggestions, queued hints, and footer/status props on presentation view models before render. |
| Rewind parity | Rewind intentionally mirrors TS `components/MessageSelector.tsx` for visible row labels, restore-option ordering, cancel paths, summarize feedback handling, and file-history metadata. Rust keeps a four-phase state machine for ratatui mechanics. Per-row `+X -Y` is computed by the CLI driver from `coco_context::FileHistoryState` snapshot pairs (TS reads `msg.toolUseResult.structuredPatch`; coco_messages has no typed tool-output side channel) and rides on `TuiOnlyEvent::RewindRowMetadataReady`. Selected restore preview rides on `RewindRestorePreviewReady`. | Treat TS `MessageSelector` as the behavior source of truth; use `codex-rs/tui` only for terminal mechanics. |
| Slash command parity | `ui/slash-command-audit.md` accounts for TS command names as implemented, compatibility-thinned, backend-specific, or deliberately omitted. `/help` includes provider-neutral commands such as `/output-style`, `/sandbox`, `/session`, `/usage`, `/add-dir`, `/doctor`, and `/hooks`. | Keep command-surface gaps explicit in the audit and `commands/CLAUDE.md` instead of treating TS-only commands as TUI bugs. |
| Command dialogs | Wired dialogs are rewind, memory, and model. Dormant plugin/MCPB/confirm `DialogSpec` variants have no current built-in producers; if produced, the dispatcher emits a transcript-visible `dialog_pending` status. | Add typed modals when a real producer appears; do not leave silent gaps. |

## Testing

Current TUI tests are colocated companion files, not inline `#[cfg(test)]`
modules. Snapshot tests use `insta` and the native-surface test renderer over
ratatui `TestBackend`.

Useful commands from `coco-rs/`:

```bash
just quick-check
cargo test -p coco-tui
cargo insta pending-snapshots --manifest-path app/tui/Cargo.toml
cargo insta accept --manifest-path app/tui/Cargo.toml
```

For documentation-only edits, source and stale-content `rg` checks are normally
enough. Rust checks are required when code changes accompany the docs.

## Maintenance Rules

- Keep this file descriptive, not aspirational.
- Name canonical source files instead of copying long enums.
- Do not record volatile file counts or line counts.
- Use `ui/agent-console-design.md` for final agent-console architecture.
- Use `ui/terminal-surface-design.md` for native terminal-surface constraints.
- Use `ui/migration-roadmap.md` only for historical implementation notes.
- Use `commands/CLAUDE.md` for slash-command parity and deliberate omissions.
- Use `ui/rendering-hardening-and-rollback.md` for rendering-layer hardening
  (cursor pin and suspend-resume) and for the rollback record of the stock
  ratatui inline viewport experiment.
- Use `ui/native-scrollback-architecture.md` for the real native-scrollback target
  architecture: custom terminal, history insertion, source-backed resize
  reflow, and modal surface ownership.
- Keep multi-LLM facts structured around provider id, API, model id,
  `ModelRole`, and `ReasoningEffort`.
