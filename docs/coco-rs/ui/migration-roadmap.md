# TUI Historical Migration Notes

Status: historical implementation notes. Not the final product organization.

`docs/coco-rs/crate-coco-tui.md` is the current implementation baseline. This
document captures broader migration notes, surface-by-surface sequencing, and
acceptance criteria. The final product target lives in
`agent-console-design.md`; the native terminal-surface constraints live in
`terminal-surface-design.md`. These notes use the current `coco-rs/app/tui`
implementation as the baseline, the TS project as a behavior reference,
`codex-rs/tui` as a ratatui terminal-mechanics reference, and
`codex-rs-tui-comparison.md` as the visible UI design comparison. TS file paths
are relative to the TS project's `src/` directory.

Do not extend this file to describe the final console. New final console design
content belongs in `agent-console-design.md`, organized by final console
surface or system boundary.

The `I*` sections below are archived implementation notes. They are useful for
understanding how earlier migration work was decomposed, but they are not the
current execution plan and must not override the evidence gates in
`agent-console-design.md` or `terminal-surface-design.md`.

## Authority

| Document | Authority |
|---|---|
| `crate-coco-tui.md` | Current TUI state, current APIs, current technical debt. |
| `ui/agent-console-design.md` | Complete target agent-console architecture on TEA / `AppState`; product-scope authority. |
| `ui/terminal-surface-design.md` | Native-scrollback terminal surface and rendering-invariant authority. |
| `ui/migration-roadmap.md` | Historical implementation notes; not final product organization. |
| `ui/codex-rs-tui-comparison.md` | Deep comparison of `codex-rs/tui` visible UI capability versus coco target boundaries. |
| `coco-rs/app/tui/CLAUDE.md` | Crate-local engineering conventions. |
| `coco-rs/commands/CLAUDE.md` | Slash-command parity, deliberate command omissions, deferred command gaps. |

When this document names something for deletion, it is target-state language.
Do not use this file to define final UX structure; use
`agent-console-design.md` for that.

## Goal

Converge the TUI on a single conversation workspace with explicit presentation
boundaries:

```text
CoreEvent / terminal input
        -> update
        -> AppState
        -> presentation view models
        -> ratatui widgets
```

`AppState` remains the source of facts. View models turn facts into rows,
labels, badges, scroll windows, and semantic style intents. Widgets render view
models. Effects such as opening editors, applying model changes, writing
settings, or responding to permission prompts stay in update/runner/service
code, never in render code.

The visible workspace should be organized as:

1. Header and status surface.
2. Conversation transcript.
3. Turn activity surface.
4. Input/composer surface.
5. Overlay layer.

## Non-Goals

- Do not port TS React/Ink component structure one-for-one.
- Do not copy Anthropic/Claude-only account, billing, mobile, desktop, Slack,
  GitHub App, or CCR flows unless `coco-rs` has a provider-neutral runtime
  capability for the same behavior.
- Do not re-enable mouse capture. Native terminal selection remains preferred.
- Do not embed raw OSC 8 hyperlink escapes in ratatui spans until ratatui has a
  native hyperlink primitive.
- Do not add display-only model strings that bypass `ModelRole` and structured
  provider/model bindings.

## Multi-LLM Invariants

These are hard constraints for all TUI work:

- Production model picker rows come from `SessionState.model_catalog`.
- Production provider availability comes from `SessionState.provider_statuses`.
- Current role bindings come from `SessionState.model_by_role`.
- Model selection emits `UserCommand::SetModelRole { role, provider, model_id,
  effort }`.
- No production UI infers provider from model id prefixes.
- No production UI hardcodes Anthropic model families as control flow.
- Role selection goes through `ModelRole`; add a role variant instead of adding
  a raw string field.
- Provider/model state is carried as structured provider id, provider API, model
  id, display name, role, and reasoning effort.
- If a current binding is absent from the catalog or filtered allowlist, the
  picker renders a synthetic current row instead of hiding it.
- Unavailable provider/model state remains typed until render. Do not collapse
  it to a boolean before footer and row badges are built.

## TS Mirror Contract

Use TS as a behavior reference only.

TS file paths in this section are relative to the TS project's `src/`
directory. Important anchors:

- `components/PromptInput/inputModes.ts`
- `components/PromptInput/PromptInput.tsx`
- `components/PromptInput/PromptInputFooter.tsx`
- `components/PromptInput/PromptInputFooterSuggestions.tsx`
- `components/memory/MemoryFileSelector.tsx`
- `utils/handlePromptSubmit.ts`
- `hooks/useExitOnCtrlCD.ts`
- `hooks/useExitOnCtrlCDWithKeybindings.ts`
- `components/MessageSelector.tsx`
- `components/ModelPicker.tsx`
- `components/permissions/PermissionPrompt.tsx`
- `screens/REPL.tsx`
- `services/compact/compactWarningHook.ts`

Mirror provider-neutral, user-visible behavior:

- key flow and focus order
- default focus and current-row visibility
- cancel/rollback semantics
- loading, empty, disabled, success, and failure states
- transcript-visible command results
- footer and picker hints
- row labels, descriptions, and preview behavior
- narrow-terminal behavior

Do not mirror:

- React component ownership
- Ink renderer structure
- GrowthBook/Statsig gates
- Anthropic account, billing, OAuth, CCR, or product-install paths
- Claude-specific command names unless `coco-rs` intentionally exposes a
  compatibility command

When `coco-rs` intentionally diverges, record the reason in this document or
the relevant crate note.

## Conflict Register

This table records known contradictions between the old docs, current source,
and target state. Terms in backticks may be stale outside this section and the
deletion plan.

| Item | Current source truth | Target state | Migration phase | Acceptance |
|---|---|---|---|---|
| Old state sample listed `plan_mode: bool` | `SessionState` derives plan mode from `permission_mode == PermissionMode::Plan`. | Keep plan mode derived from permission mode. | D1 complete; code invariant ongoing. | Docs and new APIs do not add a separate plan boolean. |
| Old state sample listed `file_suggestions` and `skill_suggestions` | `UiState` uses one `active_suggestions: Option<ActiveSuggestions>`. | Preserve a single suggestion state machine. | D1 complete; I10 preserves. | No new per-kind suggestion state. |
| Old command sample listed `MouseScroll` / `MouseClick` | `TuiEvent` has no mouse variant and `terminal.rs` never enables mouse capture. | Mouse capture remains disabled. | D1 complete; I3a codifies. | No `EnableMouseCapture`; mouse events are dropped defensively. |
| Memory prefix path | Removed. TS `components/PromptInput/inputModes.ts` recognizes only `!` as an input mode character. | Keep `/memory` as the only memory editing entrypoint. | I1 complete | Leading `#` text is ordinary chat; deleted APIs have no production call sites. |
| Direct memory append | Removed. The input bar no longer appends directly to `CLAUDE.md`, and `/memory` open results are transcript-visible. | Keep memory file edits routed through `/memory` and the editor/opener service. | I1/I6 complete for current file-row flow; I7 payload shape complete | No input-bar file writes. |
| Memory dialog payload | `OpenMemoryDialog` carries row-kind-aware file/folder/toggle rows; current command producer emits file rows. | Keep renderer and selection behavior keyed by row kind. | I7 complete for payload shape | Renderer consumes typed rows, not path/label/scope-only rows. |
| Render reads runtime helper | Removed. Coordinator display mode is resolved into `UiState` by the CLI runner before render. | Display decisions are folded into state/view model before render. | I2/I9 complete for coordinator mode | Render is deterministic from state plus frame area. |
| Input rendering duplication | Removed. `widgets/input.rs` owns `InputRenderModel` and composer rendering; `surface::viewport` only wires state into `InputWidget`. | One input view model and renderer owns composer presentation. | I10 complete for composer | Duplicate rendering path removed. |
| Direct theme reads | Overlay frame/content helpers, composer, footer/toast/activity, lifecycle banners, stash/queue/suggestion widgets, teammate header, request/confirm/model/picker/settings presentation, rich transcript/chat, markdown/diff rendering, and specialist panels use semantic `UiStyles`. Direct `Theme` access is limited to theme loading/state and the top-level `UiStyles::new(&state.ui.theme)` adapter, plus tests that create default themes. | New and migrated surfaces depend on `UiStyles`; add semantic accessors instead of passing `Theme` into renderers. | I2/I3d cleanup complete for widget/overlay-content style coupling | No direct `theme.X` reads in production renderers. |
| Chat display-collapse reducers | `transcript_presentation` handles source-backed committed cells plus active streaming/busy-tail cells. `presentation::streaming` owns active streaming-tail blocks from `StreamingState`. `transcript_projection` owns committed tool batches, same-hook lifecycle batches, parseable task-notification rendering, completed background-bash batches, and in-process teammate shutdown batches. Transcript/show-all mode keeps task notifications expanded. Read/search grouping still waits for structured read/search metadata in Rust messages. | Keep display-collapse reducers in presentation transcript cells, not widget render arms. | Projection switch complete for current Rust message shapes; active tail and streaming-tail block switch complete | Main chat consumes presentation cells and streaming blocks instead of owning reducer logic. |
| Model fallback inference | Removed. `update/show.rs` reads `SessionState.model_catalog` and provider-status rows only. | Production uses session catalog; tests/pre-bootstrap mocks seed catalog entries explicitly. | I4 complete | Production path has no model-prefix provider inference. |
| `/fast` TS command | `commands/CLAUDE.md` marks TS `/fast` deliberately not ported. TUI has `ToggleFastMode` and runtime `FastModeState`. | Do not add TS `/fast` account/product flow. Expose provider-neutral fast state only where runtime supports it. | I11 | Command audit marks omission or provider-neutral replacement. |
| `/login`, `/logout` | Deliberately omitted as Anthropic account flows. | No provider-generic login surface until provider crates expose one. | I11 | Audit marks deliberate omission. |
| TS `terminalSetup` / `/terminal-setup` | Deliberately omitted as Anthropic `claude` CLI binding installer. | No direct port. | I11 | Audit marks deliberate omission. |
| Pending command dialogs | Wired dialogs are rewind, memory, and model. Dormant plugin/MCPB/confirm `DialogSpec` variants have no current built-in producers; if produced, the dispatcher emits a transcript-visible `dialog_pending` status. | Add typed overlays when a real producer appears; do not leave silent gaps. | I11 documented | No silent dialog gaps. |
| OSC 8 links | ratatui spans do not support native hyperlinks. | Deferred. | I3a | No raw escape workaround in render code. |

## Public Interface Classification

### Current Long-Term Interface

Keep:

```rust
UserCommand::SetModelRole {
    role: ModelRole,
    provider: String,
    model_id: String,
    effort: Option<ReasoningEffort>,
}
```

This remains the model-switching boundary for the TUI.

### Target Additions or Extensions

Add or expand:

- `UiStyles`
- presentation view models
- `PickerScaffold`
- `FuzzyPicker`
- `OverlayFrame`
- `MemoryPickerRow`
- row-kind-aware `OpenMemoryDialog` payload (added)
- external editor/open intent and service boundary
- typed permission/request/MCP view models
- transcript/activity/diff/pager view models

### Migration-Only Compatibility

These exist today but should not be documented as long-term design:

- legacy memory prefix APIs listed in the memory deletion plan
- path/label/scope-only memory dialog rows
- model-prefix provider inference fallback (removed)

## Memory Deletion Plan

I1 removed these target-deleted surfaces together:

- `PromptMode::Memory`
- `UserCommand::SubmitMemory`
- `TuiOnlyEvent::MemorySaved`
- `MessageContent::MemoryInput`
- legacy help text that suggests `#note`
- prompt-mode direct append to `CLAUDE.md`

Acceptance:

- `#` at the start of input is ordinary chat text.
- TS parity is restored for prompt modes: only `!` is an input mode prefix.
- `/memory` is the only memory editing entrypoint.
- Memory edit success, cancellation, and failure are transcript-visible command
  results. Toasts may duplicate but cannot be the only feedback.

## Target Presentation Modules

Add a pure presentation layer under `app/tui/src/presentation/` as migration
work needs it:

```text
presentation/
  mod.rs
  styles.rs
  layout.rs
  renderable.rs
  surface_stack.rs
  picker.rs
  footer.rs
  notifications.rs
  dialogs.rs
  model_picker.rs
  memory_picker.rs
  permissions.rs
  conversation.rs
  streaming.rs
  pager.rs
  activity.rs
  input.rs
  suggestions.rs
  status.rs
  diff.rs
  terminal.rs
  command_surfaces.rs
```

This list is a target map, not a mandate to create empty modules. Add modules
when a phase needs them.

## Codex-RS TUI Capability Transfer Plan

See `codex-rs-tui-comparison.md` for the deeper UI design analysis. This
section keeps only the implementation transfer map.

`codex-rs/tui` is useful for two different reasons that must stay separate:

- terminal mechanics: custom terminal, native history insertion, resize reflow,
  suspend/resume, cursor style, and alt-screen overlays;
- visible UI capability: composer stack, popups, history cells, markdown/table
  rendering, diff/pager, status surfaces, pickers, and peripheral integrations.

The first group informs `native-scrollback-architecture.md`. The second group
is mostly product capability that `coco-rs` probably needs, but it should be
rebuilt through coco state, commands, config, and provider-neutral model roles.
Copy behavior, fixtures, and edge cases; do not copy module ownership blindly.

| Capability family | Codex reference | Coco need | Build plan | Priority |
|---|---|---|---|---|
| Renderable/presentation primitives | `render/renderable.rs`, `style.rs`, `ui_consts.rs` | Shared measurement, cursor claims, and by-reference rendering for cells, overlays, and bottom pane. | Add `presentation::renderable` and `UiRenderable`; use ratatui `unstable-widget-ref` and `unstable-rendered-line-info` behind coco traits. Cursor style remains a surface concern. | P0/P1 |
| Native transcript/history cells | `history_cell/*`, `transcript_reflow.rs`, `app/resize_reflow.rs` | Finalized messages, tool results, plans, approvals, patches, searches, and notices all need stable transcript rows and native scrollback replay. | Define typed transcript cell view models from `SessionState.messages`; render finalized cells to both transcript rows and history insertion lines. Source-backed replay is mandatory before native scrollback is complete. | P0 |
| Bottom pane and local surface stack | `bottom_pane/mod.rs`, `bottom_pane_view.rs`, `chat_composer/*`, `textarea.rs` | Composer, queued input, paste bursts, suggestions, approval prompts, request-user-input, and MCP elicitation must coexist without losing input state. | Build `presentation::surface_stack` over existing `Overlay` priority. Local key handling order is focused surface, keybinding context, composer, global. Keep composer state outside transient views. | P1 |
| Suggestions and command popups | `command_popup.rs`, `file_search_popup.rs`, `skill_popup.rs`, `mentions_v2/*` | Slash commands, file search, skill selection, mentions, and queued-command hints need one focus and row model. | Use `FuzzyPicker` / `PickerScaffold` with typed row kinds. Back rows by commands, skills, file-search utilities, and session state rather than codex-specific globals. | P1 |
| Permission, question, and MCP prompts | `approval_overlay.rs`, `request_user_input/*`, `mcp_server_elicitation/*` | Tool permissions, user questions, and MCP elicitation are core agent workflows. | Build typed prompt view models with options, selected index, cancel semantics, classifier state, and sticky footer. Small prompts stay inline when scrollback context matters; long content uses pager. | P1 |
| Markdown, streaming, wrapping, and tables | `markdown.rs`, `markdown_render.rs`, `markdown_stream.rs`, `table_detect.rs`, `wrapping.rs`, `line_truncation.rs` | Assistant output, tool output, plans, and docs need consistent wrapping and readable tables during streaming and after commit. | Keep existing coco markdown widgets as baseline, then add source-backed rendered-line tests. Use ratatui line measurement for display height, not as the transcript source. | P1/P2 |
| Live tool/activity surfaces | `exec_cell/*`, `chatwidget/tool_lifecycle.rs`, `unified_exec_footer.rs`, `status_surfaces.rs` | Running tools, subagents, background tasks, plans, and stream stalls should appear as one live activity surface, then commit as transcript cells. | Add `TurnActivityView` from query/task/tool state. Avoid a persistent right rail; finalized activity becomes transcript/history rows. | P1/P2 |
| Diff, pager, and review surfaces | `diff_model.rs`, `diff_render.rs`, `pager_overlay.rs` | Large diffs, plans, help, status detail, and transcript review need scrollable views without corrupting native history. | Add shared `pager` and `diff` view models. Large read-only surfaces temporarily enter alt-screen and restore the inline viewport on close. | P2 |
| Pickers, settings, and setup surfaces | `selection_list.rs`, `theme_picker.rs`, `resume_picker.rs`, `keymap_setup/*`, `status/*` | Model, memory, theme, keybinding, session resume, status, plugin, and config surfaces need common list semantics. | Reuse `PickerScaffold`, typed rows, preview state, and footer hints. Production model rows must come from `SessionState.model_catalog` and `ModelRole`. | P2 |
| Terminal peripherals | `clipboard_copy.rs`, `clipboard_paste.rs`, `external_editor.rs`, `terminal_title.rs`, `terminal_palette.rs`, `notifications/*` | Clipboard, paste classification, editor handoff, title updates, palette/style setup, and notifications are necessary for a polished TUI. | Keep effects behind service boundaries. Renderers emit intents; runner/update code performs clipboard/editor/title operations and records transcript-visible outcomes when needed. | P2 |
| Product-specific or low-ROI surfaces | `onboarding/*`, `voice.rs`, `realtime.rs`, `pets/*`, account-specific status views | Some flows are useful only when coco has matching provider-neutral runtime support. | Do not port directly. Revisit only after the runtime exposes a provider-neutral capability and config path. Decorative surfaces are out of scope. | P3 |

Implementation sequencing:

1. Build the P0 substrate first: `SurfaceTerminal`, typed transcript cells, and
   source-backed history replay.
2. Add P1 interactive surfaces next: renderable primitives, bottom pane stack,
   prompt surfaces, suggestions, and markdown/streaming hardening.
3. Add P2 breadth after the core loop is stable: diff/pager, settings pickers,
   resume/status surfaces, and terminal peripherals.
4. Keep P3 out of the migration unless a later product requirement promotes it.

Ratatui feature boundary:

- The canonical feature policy lives in
  `native-scrollback-architecture.md#ratatui-feature-policy`.
- This historical transfer map should not repeat the feature table; when in
  doubt, native scrollback and terminal API decisions defer to that document.

Acceptance for each transferred capability:

- The source of truth is `AppState` / `SessionState`, not terminal contents or
  widget-local business state.
- Rendering consumes typed view models and semantic styles.
- Provider-specific behavior does not leak into generic UI control flow.
- Native scrollback, resize, suspend/resume, narrow terminals, and focus/cancel
  behavior have snapshots, unit tests, VT100 tests, or terminal-matrix notes
  proportional to risk.

## Phase D0-D2: Documentation

| Phase | Work | Acceptance |
|---|---|---|
| D0 | Audit current docs and current source. | Authority relationship is explicit. |
| D1 | Rewrite `crate-coco-tui.md` as current-state baseline. | Current state no longer contains stale copied enums or outdated state samples. |
| D2 | Rewrite this document as target design and roadmap. | Conflicts, target APIs, multi-LLM invariants, and command boundaries are explicit. |

## Phase I0: TS Behavior Inventory

Before rewriting an interactive surface, capture the TS behavior to mirror:

- keybindings and focus order
- default focus/current-row behavior
- Enter, Tab, Shift-Tab, Escape, Ctrl-C, Up/Down, PageUp/PageDown behavior
- loading, empty, disabled, cancel, success, and failure text
- transcript result versus toast-only feedback
- preview and rollback behavior
- narrow-terminal behavior

Acceptance:

- Each migrated interactive command has a short behavior note or fixture.
- Intentional divergences are visible before implementation.
- Tests are named for behavior, not private implementation details.

## Phase I1: Remove Memory Prefix Mode

Touch points:

- `app/tui/src/state/ui.rs`
- `app/tui/src/update/edit.rs`
- `app/tui/src/command.rs`
- `app/cli/src/tui_runner.rs`
- `common/types/src/event.rs`
- `app/tui/src/state/session.rs`
- `app/tui/src/widgets/chat/render_user.rs`
- `app/tui/src/overlay_content/help.rs`
- locale strings and companion tests

Acceptance:

- Leading `#` is normal input.
- No prompt-mode append to `CLAUDE.md`.
- `/memory` remains available.
- Deleted compatibility types are absent from production paths.

## Phase I2: Presentation Boundary and Minimal UiStyles

Add the first minimal presentation boundary:

- `presentation/styles.rs`
- a small `UiStyles` facade over the existing `Theme`
- one or two presentation view models for new/migrated surfaces

This phase establishes the boundary and the public `UiStyles` facade. It does
not force rich transcript/chat, markdown, diff, or specialist panels through
the facade until their semantic style tokens are named explicitly.

Acceptance:

- New UI code and migrated chrome/overlay/input surfaces use semantic style
  methods.
- Existing custom themes and hot reload still work.
- No new `Theme` field is added unless existing fields cannot express the
  semantic intent.

## Phase I3: Shared Presentation Primitives

Split the primitive work into committable subphases.

### I3a: Layout and Terminal Helpers

- positive-width guards
- Unicode-width-aware truncation
- terminal capability resolver
- keybinding fallback table
- mouse capture explicitly remains off
- OSC 8 hyperlinks remain deferred

Acceptance:

- Narrow/normal/wide snapshots for touched surfaces.
- No render code receives zero-width wrap regions.

### I3b: Renderable Contract

Add a small measurement/render trait for complex presentation surfaces:

```rust
pub trait UiRenderable {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }
    fn desired_height(&self, width: u16) -> u16;
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn cursor(&self, area: Rect) -> Option<UiCursor> { None }
}
```

Acceptance:

- Two or three simple transcript cells migrate first.
- Measure and draw use the same wrapping assumptions.

### I3c: Surface Stack

Add a focused transient surface stack that reconciles with the existing overlay
priority queue.

Rules:

- `Overlay` remains domain state and priority authority.
- The surface stack routes focus and local keys.
- Composer text, cursor, pasted ids, images, stash state, and queue state
  survive while a surface is active.
- Key routing order is focused surface, keybinding context, composer, global.

Acceptance:

- Existing `Overlay::priority` tiers and `MAX_OVERLAY_QUEUE` behavior are
  preserved.
- Ctrl-C/Escape remain local to confirmation surfaces when appropriate.

### I3d: Footer, Notifications, Dialog Frame, Style Coverage

- pure footer props with width-collapse priority
- notification display rows and severity style intents
- shared dialog frame
- expanded `UiStyles` coverage for chrome, intent, picker, footer, and dialog
  semantics used by these primitives

Acceptance:

- Footer hints do not overlap on narrow terminals.
- Notification delivery remains in `widgets/notification.rs`.
- Dialog/picker primitives, composer chrome, lifecycle banners, toast/footer,
  activity surfaces, transcript/chat, markdown/diff, and specialist panels do
  not read `Theme` directly.

## Phase I4: Multi-LLM Model Picker

Target behavior:

- role axis: Main, Fast, Plan, Explore, Review, HookAgent, Memory, Subagent
- `ModelRole::Subagent` is the default LLM role used to run subagents when the
  spawn does not select a more specific role
- model axis: provider-grouped entries
- effort axis: supported reasoning efforts for the focused model
- typed unavailable rows
- synthetic current rows when needed
- no production provider inference from model id
- fast-mode state visible in footer/hints when relevant

Acceptance:

- Picker rows come from `SessionState.model_catalog`.
- Provider status comes from `SessionState.provider_statuses`.
- Current state comes from `SessionState.model_by_role`.
- Confirm emits `UserCommand::SetModelRole`.
- Tests cover absent-current-row preservation, unavailable reasons, effort
  downgrade, and filtered selection.

## Phase I5: Typed Permission, Request, and MCP Surfaces

Move permission and request surfaces from string-body overlays toward typed
view models:

- tool-specific permission views
- plan entry/exit approval
- request-user-input / AskUserQuestion forms
- MCP elicitation forms
- long content through pager/sticky footer

Acceptance:

- Shared prompt mechanics support options, feedback, cancellation, and
  classifier state.
- Escape/Ctrl-C cancel locally where TS behavior does.
- Long diffs and plans remain reviewable without hiding response options.

## Phase I6: External Editor and Opener Service

Replace direct editor spawning with one boundary that:

- resolves `$VISUAL || $EDITOR || vi`
- parses command and arguments with a real parser
- suspends raw mode and alternate screen when needed
- waits for terminal editors
- restores the TUI
- returns structured success/cancel/failure results

Acceptance:

- Widgets and renderers never spawn processes.
- Prompt editor, plan editor, `/memory`, and future opener flows share the
  service.
- Results can be rendered in transcript and toast surfaces.

Current implementation status:

- `/memory` file rows, prompt external-editor requests, and plan-editor
  requests enter through `UserCommand` and are executed by the CLI runner.
- the CLI runner requests a foreground terminal handoff before launching an
  editor; the TUI App leaves raw mode / alt-screen, ACKs readiness, suppresses
  terminal polling while the editor is active, and restores TUI modes before
  applying the completion event.
- prompt editor completion updates the input buffer through a TUI event.
- plan editor requests resolve the current session's concrete plan file from
  `config_home`, `project_dir`, `plans_directory`, and `session_id`, then open
  that file through the same terminal handoff.
- blocking terminal editors are waited on before `/memory`, plan-editor, or
  prompt-editor completion is emitted.

## Phase I7: Rebuild Memory Picker

Expand the dialog/event payload before migrating the renderer:

```rust
pub enum MemoryPickerRow {
    File {
        scope: MemoryScope,
        path: PathBuf,
        label: String,
        description: String,
        exists: bool,
        depth: usize,
        imported_from: Option<PathBuf>,
    },
    Folder {
        kind: MemoryFolderKind,
        path: PathBuf,
        label: String,
        description: String,
    },
    Toggle {
        kind: MemoryToggleKind,
        label: String,
        description: String,
        enabled: bool,
        editable: bool,
    },
}
```

Minimum rows:

- managed memory file entries
- user memory entry, including missing-file state
- project memory entry, including missing-file state
- project-local entry where applicable
- subdirectory/imported child entries with indentation/depth
- auto-memory folder/toggle rows when the runtime exposes them

Acceptance:

- Renderer consumes `MemoryPickerRow`.
- File creation uses create-exclusive semantics.
- Folder selection creates missing folder before open when needed.
- Success, cancellation, and failure are transcript-visible.
- `/memory refresh` or list-only compatibility is either removed or folded into
  the picker flow.

## Phase I8: Conversation, Diff, and Pager

Add shared view models for:

- committed transcript cells
- active streaming tail
- full transcript pager
- help/status/detail pagers
- structured diff rows

Acceptance:

- Committed history and active streaming have distinct contracts.
- Resize re-renders from source, not from previously wrapped lines.
- Large diffs use pager/sticky footer instead of overflowing the active
  overlay.

Current implementation status:

- structured diff parsing and old/new line-number progression live in
  `presentation::diff::DiffLineView`.
- committed transcript cells and active tail state live in
  `presentation::transcript` before ChatWidget renders them.
- chat file-edit rows and full-screen diff overlays share
  `widgets::diff_display` rendering from that view model.
- diff overlays, transcript overlays, and task-detail overlays share
  `presentation::pager::PagerWindow` for clamped ranges and title/footer
  position suffixes.

## Phase I9: Unified Activity Surface

Add `TurnActivityView` for live state:

- thinking activity
- queued/running tools
- tool progress
- subagent tree
- background tasks
- plan/todo progress
- stream stall or interrupt state

Acceptance:

- The inline activity area above the composer renders one activity view model;
  no permanent side panel or right rail remains in the target UI.
- Top-level render no longer chooses between business-specific panels.
- Feature/env/config decisions are resolved before render.

Current implementation status:

- Complete for the native viewport renderer. `presentation::activity::TurnActivityView`
  owns width-aware row capping, plan/agent/activity selection,
  stream/interrupt status, tool activity, subagent/coordinator rows, and
  plan/todo/background-task rows.
- `render_main_area` keeps one main workspace; `render_chat_and_input` places
  `widgets::ActivityPanel` inline above the composer, mapping the single
  activity view model to ratatui spans without a right-side rail.

## Phase I10: Input, Suggestions, and Footer Unification

Collapse input rendering and suggestion/footer logic:

- one input view model
- one input renderer
- one suggestion row model
- pure footer props
- prompt suggestions and queued command hints use the same focus model

Acceptance:

- Prefix stripping and visual cursor math live in one place.
- The legacy memory prefix cannot reappear accidentally.
- Pasted content, images, stash, history, queued input, and suggestions are
  tested together.

Current implementation status:

- `widgets/input.rs` owns the composer render model and visible prefix math.
- `presentation::input::InlinePopupView` owns autocomplete/command-palette
  popup rows and selection before `render_chat_and_input` sizes the popup slot.
- `presentation::footer::FooterView` owns status-bar/exit-prompt props,
  including model display, effort, permission mode, tokens, context, MCP, chord
  hints, and message counts.

## Phase I11: Slash Command Surface Parity

Interactive slash commands should return typed command surfaces and
transcript-visible command results when `coco-rs` has an equivalent backend.

Mirror or implement provider-neutral UI when supported:

- `/model`, `/memory`, `/theme`, `/config`, `/output-style`
- `/permissions`, `/sandbox`, `/mcp`
- `/agents`, `/tasks`, `/session`, `/resume`
- `/context`, `/status`, `/diff`, `/export`, `/help`
- `/hooks`, `/plugins`, `/skills`, `/keybindings`
- `/add-dir`, `/ide`, `/rewind`, `/doctor`

Use compatibility text, deliberate omission, or backend-unsupported status for
TS/product-specific commands:

| Command | Target classification |
|---|---|
| `/login`, `/logout` | Deliberate omission until provider-neutral auth UI exists. |
| `/fast` | Deliberate omission as TS slash command; expose runtime fast state through provider-neutral TUI controls only. |
| `/terminal-setup` / TS `terminalSetup` | Deliberate omission; Anthropic `claude` CLI binding installer. |
| `/privacy-settings`, `/rate-limit-options`, `/reset-limits`, `/extra-usage` | Product/account-specific unless `coco-rs` exposes provider-neutral equivalents. |
| `/install-github-app`, `/install-slack-app`, `/mobile`, `/desktop`, `/chrome`, `/passes` | Product/platform-specific deliberate omissions. |
| `/ultraplan`, `/ultrareview`, `/advisor`, `/voice`, `/think-back`, `/thinkback-play` | Anthropic/CCR/internal or experimental backend-specific omissions. |

Acceptance:

- A checked-in command audit accounts for every TS command name as implemented,
  alias/compat-message, backend unsupported, or intentionally omitted.
- No TS-only command is treated as a missing TUI bug without checking
  `commands/CLAUDE.md`.

Current implementation status:

- Complete. `docs/coco-rs/ui/slash-command-audit.md` accounts for the TS
  command scan from `claude-code-kim/src/commands`.
- `/help` includes the provider-neutral TUI commands from this phase, including
  `/output-style`, `/sandbox`, `/session`, `/usage`, `/add-dir`, `/doctor`,
  and `/hooks`.

## Design Rules

- Render functions are pure from `AppState`/view model plus frame area.
- Widgets do not perform filesystem, process, config, network, or runtime
  side effects.
- Update and runner code own behavior and effects.
- Closed UI states use typed enums or structs, not unstructured
  `serde_json::Value`, when producer and consumer are both in `coco-rs`.
- Complex surfaces measure before rendering.
- Terminal-specific behavior is centralized.
- User-visible command outcomes are transcript-visible; toasts are not the
  only durable result.
- Use i18n keys for labels and hints.
- Use snapshots for visible UI changes.
- Keep files under local module-size guidance; split before creating another
  render dumping ground.

## Verification

Documentation-only verification:

- Run the stale-term `rg` check from the task plan against these two docs.
  Legacy terms should appear only in a conflict register or deletion plan.
- Run a source `rg` check for the current memory and model APIs named by this
  roadmap, then compare the hits against `app/tui/CLAUDE.md`,
  `commands/CLAUDE.md`, `common/types/src/event.rs`, and
  `app/cli/src/tui_runner.rs`.

Implementation-phase verification:

- TUI-only change: `just quick-check` and `just test-crate coco-tui`.
- Command/event type change: add `just test-crate coco-commands` or broader
  `just test` when shared crates are affected.
- Visible UI change: add/update focused `insta` snapshots at narrow, normal,
  and wide widths where relevant.
- Final commit gate: run `just pre-commit` once at the end.
