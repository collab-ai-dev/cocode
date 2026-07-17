# Coco Agent Console Design

Status: final product architecture for the complete `coco-rs` agent console.

This document evaluates and designs a full agent console comparable in product
scope to `codex-rs/tui`, but built on coco's TEA / `AppState` model. This is
the highest-level UI target. It is organized by the final console system, not by
temporary delivery order.

Read this with:

- `terminal-surface-design.md` for terminal-surface constraints;
- `native-scrollback-architecture.md` for terminal mechanics;
- `codex-rs-tui-comparison.md` for the reference UI analysis;
- `crate-coco-tui.md` for current implementation facts.

This document supersedes the phased migration notes for final product
organization. New design work should be added here by final console surface or
system boundary.

Authority rules:

- Product scope, state ownership, surface placement, activity layout, and effect
  flow are canonical in this document.
- Ratatui feature policy, `SurfaceTerminal`, native history insertion, resize
  replay, Zellij behavior, and scrollback truncation UX are canonical in
  `native-scrollback-architecture.md`.
- `terminal-surface-design.md` is a checklist for terminal-surface invariants.
- `rendering-hardening-and-rollback.md` is historical evidence and regression
  guardrail, not a competing target design.

## Evaluation

Building the final console on TEA / `AppState` is the right architecture for
`coco-rs`, with two conditions:

1. `AppState` remains the source of facts, not a store of rendered rows.
2. Presentation and terminal side effects are explicit layers below state.

The viable shape is:

```text
terminal input / CoreEvent / timers
        -> event normalization
        -> update reducers
        -> AppState
        -> presentation view models
        -> surface renderer
        -> terminal backend
```

This gives coco the agent-console capability of `codex-rs/tui` while preserving
multi-provider correctness, testability, and clean command boundaries.

The risk is not TEA itself. The risk is allowing one of these layers to absorb
the others:

- if `AppState` stores rendered line caches as source truth, resize and replay
  become fragile;
- if widgets perform effects, tests become nondeterministic;
- if `SurfaceTerminal` leaks upward, ordinary UI code starts writing terminal
  escape sequences;
- if presentation models are skipped, `surface::viewport` or overlay renderers
  grow into untestable mega-widgets.

The design below avoids those failure modes.

## Required Evidence Before Broad Implementation

These are not product phases. They are risk gates that must be answered before
large UI rewrites or broad terminal integration land.

| Gate | Question | Required evidence | Time-box / owner / kill switch | Decision if it fails |
|---|---|---|---|---|
| Terminal surface spike | Can the useful parts of `codex-rs/tui/src/custom_terminal.rs` compile against crates.io `ratatui 0.30` without the old fork's private affordances? | A small `SurfaceTerminal` proof that owns viewport area, cursor position/style, diff invalidation, visible-history row accounting, and can run with buffer snapshots plus VT100 byte tests. | 2 weeks. Owner: TUI architecture owner. Re-evaluate at 4 weeks. | Decide explicitly between a coco-owned ratatui fork, a narrower native-scrollback target, or deferring native scrollback. Do not discover this after presentation work depends on it. |
| Source-backed stream controller | Can stable/tail streaming, table holdback, and width-change rerendering be implemented as pure logic over source text? | Unit tests for append, partial line, markdown/table holdback, resize, finalization, and emitted-stable accounting, independent of terminal rendering. | 2 weeks. Owner: streaming/presentation owner. Re-evaluate at 4 weeks. | Keep history emission source-backed; do not reintroduce the deleted fullscreen fallback. |
| Activity layout | Can `TurnActivityView` stay readable under real concurrency instead of becoming a new mega-widget? | `<=60`, narrow, normal, and wide snapshots for light and heavy turns, including subagents plus background tasks plus a long-running tool. | 2 weeks. Owner: console product owner plus TUI architecture owner. Re-evaluate at 4 weeks. | Split activity into typed groups and pager-backed detail before wiring more producers. |

Terminal surface decision criteria:

- if owning the diff requires changing ratatui `Buffer` or cell visibility
  APIs, choose a coco-owned ratatui fork or defer native scrollback;
- if the missing pieces are only terminal plumbing around writes, cursor style,
  synchronized update, or scroll-region commands, keep crates.io ratatui and
  implement the missing terminal shell in `SurfaceTerminal`;
- if neither path can produce viewport-pinned redraw, clean clear/replay, and
  VT100-verified history insertion by the 6-week stop point, ship the improved
  in-app transcript/pager path and keep native scrollback blocked.

If the terminal surface spike cannot reach a clear decision after 6 weeks, stop
native-scrollback integration work and ship the existing base surface with
improved in-app transcript/pager UX while keeping the native design as a blocked
target. This is a stop condition, not a second product mode.

The deleted fullscreen `render.rs` proved why this is necessary. New work should
enter typed presentation boundaries, `surface/*`, or focused widgets rather than
adding branching render logic to the viewport or overlay coordinators.

## Design Comparison

There are now two design documents because they answer different questions:

| Design | Scope | Strengths | Weaknesses | Use it for |
|---|---|---|---|---|
| `agent-console-design.md` | Complete final agent console: transcript, streaming, activity, bottom pane, prompts, pickers, pagers, terminal surface, effects, performance, tests. | Product-complete; matches the actual target comparable to `codex-rs/tui`; keeps TEA / `AppState` as the organizing boundary; prevents over-optimizing only the terminal layer. | Larger architecture; requires more typed view models and clearer module ownership; higher upfront design discipline. | Any decision about final UX, state ownership, module boundaries, and what must ship. |
| `terminal-surface-design.md` | Native-scrollback terminal surface and rendering invariants. | Concrete and necessary; defines the hardest terminal mechanics clearly; easier to verify with VT100/manual terminal tests; lower ambiguity for `SurfaceTerminal`. | Not a complete agent console; underspecifies bottom pane, activity, prompt, picker, pager, and peripheral systems; can lead to a terminal-only local optimum if treated as the final product. | Terminal backend, native scrollback, cursor, resize, suspend/resume, overlay placement constraints. |

Verdict:

- `agent-console-design.md` is the final delivery design.
- `terminal-surface-design.md` is a required subsystem design under it.
- A feature is complete only when it satisfies the agent-console design and the
  relevant terminal-surface constraints.

## Product Target

The final console is a single terminal workspace for long agent sessions:

1. native terminal scrollback for finalized transcript history;
2. retained interactive viewport for live turn state and input;
3. bottom composer with suggestions, attachments, queued input, and local
   decision surfaces;
4. attention-safe inline or pager/overlay prompts for permissions, user
   questions, MCP elicitation, and plan approval;
5. dense pickers for models, sessions, commands, skills, files, memory, MCP,
   keybindings, and settings;
6. unified live activity surface for tools, subagents, hooks, tasks, plan/todo
   progress, queue state, and stream health;
7. large review surfaces for transcript, diffs, plans, search results, and
   status detail;
8. terminal-safe peripherals for clipboard, external editor, terminal title,
   notifications, focus, suspend/resume, and config/theme hot reload.

No persistent right-side execution rail is part of the target. Side panels may
exist during migration, but the final console presents live work through the
activity surface and commits completed work into transcript cells.

## Layering

### 1. Event Sources

Inputs entering the TUI:

- crossterm terminal events: keys, paste, resize, focus, suspend;
- `CoreEvent::Protocol`: session, turn, task, MCP, hook, queue, cost, rewind,
  sandbox, and system events;
- `CoreEvent::Stream`: streaming text, thinking, tool, and MCP tool-call state;
- `CoreEvent::Tui`: local overlays, toasts, command results, UI-only prompts;
- async local services: file search, symbol search, keybinding/theme/display
  hot reload, config errors;
- timers: status tick, animation tick, redraw request, idle notification.

Event normalization should produce `TuiCommand` or reducer calls. Mouse capture
remains disabled; terminal selection and wheel scroll are owned by the terminal.

### 2. Update Layer

The update layer mutates `AppState` and emits effects. Current code sends
`UserCommand` directly from `update.rs`; the target can keep that initially, but
new side-effecting console work must move through an explicit outcome:

```rust
pub struct UpdateOutcome {
    pub redraw: bool,
    pub effects: Vec<EffectRequest>,
}

pub struct EffectRequest {
    pub id: EffectId,
    pub generation: EffectGeneration,
    pub effect: TuiEffect,
    pub started_at: Instant,
    pub cancellation: CancellationToken,
}

pub enum EffectLifecycle {
    Pending,
    Running,
    CancelRequested,
    Cancelled,
    Done,
}

pub enum TuiEffect {
    SendUserCommand(UserCommand),
    OpenExternalEditor(EditorRequest),
    CopyToClipboard(ClipboardRequest),
    SetTerminalTitle(TerminalTitleRequest),
    Notify(DesktopNotificationRequest),
    RequestRedrawAfter(Duration),
}

pub struct EffectResult {
    pub id: EffectId,
    pub generation: EffectGeneration,
    pub result: Result<(), TuiEffectError>,
}

pub enum TuiEffectError {
    Cancelled,
    Failed(String),
}
```

This keeps widget renderers pure and lets tests assert intent without spawning
processes or touching the terminal. Effects that can fail or produce
user-visible state must flow back through an `EffectResult` event and be reduced
into `AppState`; fire-and-forget is only acceptable for redraw timers and other
loss-tolerant hints.

New code that opens an editor, writes the clipboard, changes the terminal title,
fires a desktop notification, requests redraw timing, or writes terminal-owned
state must emit `EffectRequest`. Existing direct `UserCommand` sends can be
grandfathered while they are being migrated, but new local side effects should
not be added directly to `update/*.rs`. The seam guard should reject new direct
clipboard/editor/title/notification calls outside the effect runner or surface
layer.

`EffectId` is unique only within a TUI generation. Reducers keep an
`in_flight_effects` table keyed by `(EffectId, EffectGeneration)` and drop stale
results when an effect was cancelled by `/clear`, rewind, session switch, or
shutdown. Ordinary overlay open/close does not bump the global generation:
closing a model picker must not invalidate an unrelated editor or clipboard
effect. Overlay-owned effects use an owner token and explicit
`cancel_in_flight_effects(owner)` if the overlay's result should be discarded.
Some OS calls, such as clipboard writes, may not be physically cancellable after
dispatch; their late result is still ignored once the effect is no longer in
flight.

Error boundaries:

- `surface/*` returns `io::Result` for backend and terminal operations;
- effect runners return typed `TuiEffectError`;
- reducers return `UpdateOutcome` and represent user-visible failures in
  `AppState`, not `Result`;
- render and presentation code stay pure and do not create errors with side
  effects.

### 3. AppState

`AppState` remains the canonical model:

```rust
pub struct AppState {
    pub session: SessionState,
    pub ui: UiState,
    pub running: RunningState,
}
```

`SessionState` stores agent-synchronized facts:

- messages and stable message ids;
- model catalog, provider status, role bindings, reasoning effort;
- permission mode and capability availability;
- tool executions, MCP status, active hooks;
- subagents, background tasks, plan/todo state;
- token/context usage, rate-limit/fallback/stream-health facts;
- saved sessions, available commands, agents, skills, memory entries;
- queued commands and queue acknowledgements.

`UiState` stores local terminal state:

- input/composer text, cursor, paste manager, stash, history;
- active suggestions;
- overlay priority queue;
- local surface stack;
- active stream controller state;
- terminal focus, hot-reload snapshots, theme/display settings;
- toasts, double-press trackers, collapsed/expanded UI preferences;
- pager scroll positions and picker selections.

Rules:

- `AppState` may store source state and UI control state.
- `AppState` must not store terminal scrollback contents as truth.
- `AppState` must not store raw terminal backend state.
- Rendered lines may be cached only with explicit source/version/width keys and
  must be disposable.

### 4. Presentation Layer

Presentation transforms `AppState` into view models:

```text
presentation/
  console.rs
  styles.rs
  layout.rs
  renderable.rs
  conversation.rs
  streaming.rs
  activity.rs
  bottom_pane.rs
  surface_stack.rs
  picker.rs
  pager.rs
  diff.rs
  prompts.rs
  status.rs
  terminal.rs
```

Presentation owns:

- semantic style mapping;
- layout guards and width-safe truncation;
- transcript cell construction;
- picker row grouping/filtering/preview state;
- activity row construction;
- prompt form view models;
- pager/diff row view models;
- renderable measurement.

Presentation does not:

- send `UserCommand`;
- read environment variables;
- spawn editors;
- write clipboard or terminal escapes;
- mutate session state.

The key contract is `UiRenderable`:

```rust
pub trait UiRenderable {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }
    fn raw_lines(&self) -> Vec<Line<'static>>;
    fn desired_height(&self, width: u16) -> u16;
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn cursor(&self, area: Rect) -> Option<UiCursor> { None }
}
```

Ratatui `unstable-widget-ref` and `unstable-rendered-line-info` are useful
inside this layer, behind coco-owned traits. Do not re-export ratatui unstable
types from coco presentation APIs. If those ratatui features change, only the
adapter code should change.

`UiRenderable` is a presentation adapter, not transcript storage. Do not store
`Box<dyn UiRenderable>` as canonical cell content. Transcript cells keep source
references and semantic kinds; draw code builds the concrete renderer for the
current width/context. This avoids duplicating long message text into owned
`Line<'static>` payloads and matches the codex-rs pattern of enum-dispatched
history cells rather than persistent trait objects.

Cursor arbitration is separate from rendering. Multiple surfaces may expose a
cursor claim, but exactly one claim reaches the terminal:

1. focused local surface, such as a picker filter or prompt text field;
2. active modal/overlay, which may either claim or hide the cursor;
3. composer/input;
4. base viewport, which normally hides the cursor because finalized history is
   not editable.

Security/blocking overlay priority still comes from `Overlay::priority`.
Cursor priority is the focus chain above; the final surface frame resolves the
claim after all renderables have declared intent.

Overlay-owned text fields are a compound claim, not two competing claims. The
overlay owns the cursor namespace and exposes the internally focused field's
position/style through that single claim. The composer cannot also claim the
cursor while a modal or focused local surface is active.

### 5. Surface Layer

The surface layer owns terminal state:

```text
surface/
  terminal.rs
  frame.rs
  history_insert.rs
  history_emitter.rs
  transcript_reflow.rs
  overlay_placement.rs
```

Responsibilities:

- native scrollback insertion;
- viewport geometry;
- cursor position and cursor style;
- diff invalidation;
- clear/replay on resize or `/clear`;
- alt-screen entry/exit for large overlays;
- suspend/resume repair;
- terminal matrix behavior.

No widget writes terminal escape sequences directly. If a UI needs a terminal
effect, it emits a typed effect that the runner/surface layer executes.

## Console Surfaces

### Header And Status

Purpose:

- identify current provider/model/role binding and reasoning effort;
- show cwd/git/worktree/session identity;
- show compact context/token/queue/permission hints;
- surface transient high-severity banners without stealing input focus.

Design:

- render from `ConsoleHeaderView`;
- all provider/model data comes from `SessionState.model_by_role`,
  `model_catalog`, and `provider_statuses`;
- no provider inference from model id prefixes;
- narrow width collapses from detail to badges to minimal labels.

### Transcript

The transcript is source-backed, not terminal-backed.

Core types:

```rust
pub struct TranscriptCell {
    pub id: TranscriptCellId,
    pub kind: TranscriptCellKind,
    pub source: TranscriptSourceRef,
    pub finalized: bool,
}

pub enum TranscriptCellKind {
    User,
    Assistant,
    Thinking,
    ToolCall,
    ToolResult,
    LocalCommand,
    Plan,
    PermissionDecision,
    UserInputAnswer,
    Mcp,
    Hook,
    Task,
    AttachmentNotice,
    SystemNotice,
}
```

Rules:

- finalized cells are eligible for terminal history emission;
- active cells render only inside the interactive viewport;
- raw/copy-friendly output is generated from source, not from visible rows;
- transcript pager uses the same cell model plus optional live tail;
- rewind/truncate/session switch invalidates emitted prefix and requests replay;
- image and binary attachments render in transcript history as source-backed
  placeholder rows such as `[image: name, width x height, size]`; inline graphics
  protocols such as Kitty/Sixel are future capability-gated enhancements and
  must not bypass the transcript cell contract.
- fenced code blocks may use syntax highlighting in display/transcript lines,
  gated by display settings and theme support; raw/copy output always comes
  from source text and does not depend on highlighted spans.

### Streaming

Streaming uses source-backed stable/tail partitioning:

```rust
pub struct MarkdownStableTail {
    raw_source: String,
    rendered_lines: Vec<Line<'static>>,
    mutable_tail_start: usize,
    width: Option<u16>,
    table_holdback: TableHoldbackState,
}

pub struct HistoryEmissionController {
    emitted_stable_len: usize,
    enqueued_stable_len: usize,
}

pub struct StreamController {
    markdown: MarkdownStableTail,
    history: Option<HistoryEmissionController>,
}
```

Behavior:

- append-only deltas accumulate into `raw_source`;
- only complete, stable lines enter the history queue;
- partial lines and mutable table regions stay in the live tail;
- resize re-renders from source and rebuilds queue state;
- finalization returns remaining uncommitted lines and a finalized transcript
  cell.
- finalization consumes the active `raw_source` and drops stream-only buffers
  once the finalized source-backed transcript cell exists;
- large active streams use segmented source storage: stable emitted segments
  plus a mutable tail segment, so width changes can avoid reparsing the entire
  turn when only the live tail is still mutable. The initial threshold should be
  explicit, for example 256 KiB per active stream, and tuned from observed
  resize/reparse latency.

This is mandatory for native scrollback. Without it, tables, URLs, and wrapped
markdown can mutate after rows have been emitted.

The markdown stable/tail layer is useful even if native history is blocked. The
history-emission layer exists only when stable lines are queued for native
scrollback; do not let `emitted_stable_len` / `enqueued_stable_len` become dead
state in the fullscreen fallback path.

### Turn Activity

`TurnActivityView` is the unified live-work surface.

Inputs:

- streaming controller state;
- running/queued tools;
- MCP calls;
- active hooks;
- subagent tree;
- background tasks;
- plan/todo progress;
- queued commands;
- stall/interrupt state.

Outputs:

- compact live rows in the viewport;
- detailed rows for pager/status surfaces;
- finalized transcript cells when work completes.

Rules:

- no permanent right rail in the final UI;
- activity does not duplicate already-finalized transcript rows;
- live rows are dense, stable, and width-aware;
- long tool output is summarized live and reviewable via pager/history;
- live activity rows and their final transcript cells share a stable
  `ActivityId`/`TranscriptCellId` relationship so consolidation does not make
  scroll position jump;
- each row has a collapse priority before implementation: stream tail, blocking
  prompt, running tools, subagent group, background tasks, queue, historical
  detail.

The collapse order is code, not prose. `ActivityProducerPriority` is an enum
that every producer maps to before rendering:

```rust
pub enum ActivityProducerPriority {
    BlockingPrompt,
    StreamTail,
    RunningTool,
    BlockedSubagent,
    RunningSubagent,
    BackgroundTask,
    Hook,
    Queue,
    HistoricalDetail,
}
```

Width policy:

| Width | Visible by default | Collapsed | Hidden unless pager |
|---|---|---|---|
| `<=60` | blocking prompt, one stream tail line, one aggregate running/blocked row, composer | running tools/subagents/tasks into counts | queue detail, historical detail, long output |
| `<=80` | prompt, stream tail, first running tool, subagent aggregate, composer/footer | tasks/hooks/queue into compact rows | per-subagent detail, long output |
| `~100` | prompt, stream tail, running tools, subagent groups, plan/todo progress | hooks and queue as compact groups | old completed activity |
| `>=120` | prompt, stream tail, grouped tools, grouped subagents, task/hook/queue summary, selected detail preview | only overflow groups | old completed activity |

Snapshots prove the table; the table owns the implementation priority.

Layout sketches:

All user-facing text below is a structural placeholder. The implementation uses
`tr(...)` strings and measures translated display width with `unicode-width`
before truncating; no layout may assume the English wording fits.

```text
Minimal (<=60 cols)
  tr("activity.stream.tail")
  tr("activity.compact", running=3, blocked=1)
  input

Narrow, light (<=80x24)
  assistant tail line 1...
  tool: bash running 12s
  input
  footer

Narrow, heavy
  assistant tail line 1...
  5 subagents running - 2 active, 3 collapsed
  tools: bash 5m, ripgrep 22s, tests queued
  +3 tasks, +2 hooks, details in activity pager
  input
  footer

Normal, light (~100x30)
  assistant tail, wrapped to content width
  tool bash running 12s | last output summary
  plan 2/5 done
  input
  footer

Normal, heavy
  assistant tail
  subagents: explore(2 running) plan(1 blocked) review(2 queued)
  tools: bash running 5m | tests queued | mcp search done
  tasks: build docs active | hooks 2 active | queue 3
  detail hint: open activity pager
  input
  footer

Wide, light (>=120)
  assistant tail with inline tool summary
  bash running 12s | cwd | last line
  input with suggestions/attachments
  footer

Wide, heavy
  assistant tail
  subagents grouped by type/status with first active row expanded
  tools grouped by running/queued/completed with one-line last output
  tasks/hooks/queue compact groups
  selected group preview or pager hint, not a permanent right rail
  input
  footer
```

### Bottom Pane

The bottom pane is the input and local-interaction system.

State:

```rust
pub struct BottomPaneState {
    pub composer: ComposerState,
    pub local_stack: SurfaceStackState,
    pub pending_input: PendingInputPreview,
    pub pending_approvals: PendingApprovalPreview,
    pub footer: FooterState,
}
```

Routing:

```text
focused local surface
        -> active autocomplete / picker context
        -> composer
        -> global command
```

Paste bursts are composer-owned by default. A focused local surface receives
ordinary typed keys, but it receives bracketed paste only if its view model
declares `consumes_paste() == true` and a size policy. Pickers and filters do not
receive raw multi-megabyte paste; they route it to the composer paste manager or
ignore it with an explicit local result.

Local surfaces:

- command/file/skill/agent/mention popup;
- request-user-input prompt;
- MCP elicitation form;
- permission prompt when it needs scrollback context, fits inline, and can be
  made attention-safe;
- queued input editor;
- paste burst preview;
- lightweight settings or status selectors.

Composer state is retained while local surfaces are active. Dismissing a prompt
must not drop draft text, pasted images, mention bindings, stash state, or
history search state.

Vim mode is composer-internal state inside `ComposerState`. It participates in
the keybinding context after a focused local surface has declined the key and
before normal composer editing. Pickers, prompts, and permission forms either
use their own focused-field editing rules or explicitly disable vim behavior;
they do not inherit command-mode operators by accident.

### Overlay And Pager Placement

There are three placements:

| Placement | Use for | Terminal behavior |
|---|---|---|
| Bottom-pane local | Small focused forms and suggestions that should preserve scrollback context. | Retained inline viewport. |
| Inline decision prompt | Small prompts that fit near live context and can be guaranteed visible. | Retained inline viewport. |
| Alt-screen pager | Large read-only or navigation surfaces: transcript, diff, status detail, help, session browser when large. | Enter alt-screen, restore inline viewport on close. |

`Overlay::priority` remains the authority for security and blocking prompts.
Placement is a render/surface decision attached to the overlay kind.

Blocking prompts must be attention-safe. In native-scrollback mode the terminal
cannot reliably know whether the user has scrolled the native scrollback away
from the retained viewport. Permission, sandbox, MCP elicitation, and other
blocking prompts therefore either use an attention-safe overlay path
(alt-screen/local focus plus bell/status banner) or prove that the viewport is
currently visible before choosing inline placement. Inline-only blocking prompts
are not allowed.

Visibility proof is conservative because terminals do not expose native
scrollback viewport position. The viewport is considered visible only after a
recent app-directed interaction that brings focus back to the retained surface:
a key event routed to the composer/local surface, a focus-gained event followed
by a successful draw, or an explicit user action that opened the prompt target,
within a short window such as 2 seconds. Otherwise a blocking prompt upgrades to
the attention-safe path.

### Pickers

All pickers use one scaffold:

```rust
pub struct PickerRow {
    pub id: String,
    pub group: Option<String>,
    pub label: String,
    pub description: Option<String>,
    pub badge: Option<Badge>,
    pub disabled: Option<DisabledReason>,
    pub preview: Option<PreviewRef>,
    pub action: PickerAction,
}
```

Shared behavior:

- filter;
- selected row preservation across refresh;
- tabs;
- grouped rows;
- disabled rows with reasons;
- narrow/normal/wide layout;
- footer hints;
- preview side content when width allows.

Model picker invariants:

- rows come from `SessionState.model_catalog`;
- availability comes from `SessionState.provider_statuses`;
- role binding comes from `SessionState.model_by_role`;
- `ModelRole::Subagent` is a real model role: it is the default LLM binding for
  subagent execution when a spawn does not resolve to a more specific role such
  as `Explore`, `Plan`, or `Review`;
- confirm emits `UserCommand::SetModelRole`.

### Permission And Prompt Surfaces

Prompts are typed view models:

- permission approval;
- plan entry/exit/approval;
- sandbox/network/file-system approval;
- request-user-input;
- MCP elicitation;
- cost/trust/bypass confirmation;
- idle return and interrupt decisions.

Each prompt declares:

- blocking level and priority;
- placement preference;
- response commands;
- cancellation semantics;
- whether Ctrl-C/Esc is local or global;
- optional feedback input;
- optional content attachments;
- long-content pager route.

This follows the useful `codex-rs/tui` behavior of bottom-pane approvals and MCP
elicitation, but adds the native-scrollback visibility rule that codex's retained
viewport does not need.

### Terminal Peripherals

Terminal-adjacent capabilities are effects:

- clipboard copy/paste and clipboard leases;
- external editor;
- terminal title;
- focus notifications;
- desktop notifications;
- suspend/resume;
- keyboard enhancement mode;
- bracketed paste.

Renderers do not execute these. Update emits intent; app runner or surface layer
executes and emits a result back into `AppState` when user-visible.

## Module Boundary Proposal

Add only when needed:

```text
app/tui/src/
  surface/
    mod.rs
    terminal.rs
    frame.rs
    history_insert.rs
    history_emitter.rs
    transcript_reflow.rs
    overlay_placement.rs
  presentation/
    console.rs
    renderable.rs
    conversation.rs
    streaming.rs
    activity.rs
    bottom_pane.rs
    surface_stack.rs
    picker.rs
    pager.rs
    diff.rs
    prompts.rs
    terminal.rs
  update/
    surface.rs
    activity.rs
    prompts.rs
```

Existing modules migrate gradually:

- split large coordinator code before adding new surface families; new complex
  UI work should enter `presentation/*`, `surface/*`, or a small widget module,
  not expand `surface::viewport` or generic overlay coordinators;
- `widgets/chat/*` -> `presentation::conversation` plus small widgets;
- `overlay_content/*` -> `presentation::prompts`, `picker`, `pager`, `diff`;
- `streaming/*` -> source-backed stream controller;
- `terminal.rs` -> `surface::terminal`;
- side-panel-era widgets -> `presentation::activity`; final placement is inline
  activity above the composer, not a permanent rail.

## Data Flow Examples

### Submitting Input

```text
Key Enter
  -> keybinding resolver
  -> TuiCommand::SubmitInput
  -> update::edit validates local command / paste / attachments
  -> AppState adds optimistic user transcript cell
  -> effect: UserCommand::SubmitInput
  -> CoreEvent::Protocol(TurnStarted)
  -> SessionState turn state starts
  -> presentation builds active turn view
```

### Streaming Assistant Text

```text
CoreEvent::Stream(TextDelta)
  -> stream reducer appends source
  -> StreamController partitions stable/tail
  -> stable lines queue for history emission
  -> mutable tail renders as active cell
  -> TurnCompleted finalizes remaining tail into transcript cell
```

### Permission Prompt

```text
CoreEvent::Tui/OpenPermission
  -> UiState::set_overlay applies priority
  -> presentation chooses attention-safe inline, bottom-pane, or alt-screen placement
  -> local key routing handles approve/deny/feedback
  -> effect: UserCommand::ApprovalResponse
  -> prompt result commits as transcript-visible decision
```

### Resize

```text
TuiEvent::Resize
  -> surface records size change
  -> active stream re-renders from source
  -> history emitter requests replay when width changed
  -> transcript cells remeasure at new width
  -> terminal scrollback is cleared/replayed from source-backed cells
```

## Invariants

- `SessionState` is canonical for agent facts.
- `UiState` is canonical for local control state.
- Terminal scrollback is a projection.
- Finalized transcript cells emit to history once per generation.
- Active cells never enter terminal history until finalized.
- Widget render functions are pure.
- Terminal effects go through `TuiEffect` or surface APIs.
- Provider/model UI uses structured provider/api/model ids and `ModelRole`.
- Overlay priority is separate from overlay placement.
- Resize, clear, rewind, and session switch reconcile from source.
- Blocking prompts cannot be hidden behind native-scrollback user scroll.

## Performance Model

The console must handle long sessions without rendering every row every frame.

Use:

- emitted-prefix tracking for native history;
- last-width measurement caches for transcript cells;
- live-tail cache keys for transcript pager;
- event coalescing before redraw;
- adaptive stream draining;
- capped live summaries for tool output;
- pager virtualization for large overlays.

Measurement caches are bounded and owned by `UiState`, not by individual cells:

```rust
pub struct MeasurementCache {
    entries: HashMap<(TranscriptCellId, u16), MeasuredLines>,
    budget_bytes: usize,
}
```

The default budget is 50 MiB. Ordinary transcript cells keep only the most
recent width entry; pager-visible cells may keep a short LRU while visible.
Eviction uses generation and transcript-cell age before memory growth can scale
with every width the user has tried.

Avoid:

- storing terminal rows as source;
- recomputing markdown for all cells every tick;
- cloning full transcripts for ordinary frame draws;
- rendering invisible pager rows;
- synchronous file/git scans in render.

## Testing Strategy

Tooling lock-in:

- pure update/presentation logic uses companion `*.test.rs` unit tests with
  `pretty_assertions` for structured values;
- visible ratatui surfaces use `insta` snapshots and `ratatui::backend::TestBackend`
  for buffer/cell comparison;
- native scrollback and terminal-control behavior use a byte-capturing backend
  parsed by the `vt100` crate, following the `codex-rs/tui` `VT100Backend`
  pattern; `TestBackend` does not produce ANSI bytes for vt100 parsing;
- snapshot files stay next to the tested module under `snapshots/`;
- long-session performance checks use synthetic transcript fixtures rather than
  terminal contents;
- seam guards reject new cursor-applying call sites in widgets/overlays and new
  direct clipboard/editor/title/notification calls outside the effect runner or
  surface layer.

Minimal VT100 fixture shape:

```rust
let backend = VT100Backend::new(/*width*/ 80, /*height*/ 24);
let mut terminal = SurfaceTerminal::with_options(backend)?;
terminal.insert_history_lines(lines)?;
let screen = terminal.backend().vt100_screen();
```

Use `TestBackend` for widget snapshots and diff-buffer assertions. Use the VT100
backend for DECSTBM, Reverse Index, cursor restore, alt-screen enter/leave, and
scrollback insertion behavior.

Automated:

- reducer tests for `AppState` transitions;
- presentation snapshot tests for narrow/normal/wide widths;
- transcript cell tests for display/raw/transcript lines;
- stream controller tests for stable/tail/table/resize behavior;
- surface VT100 tests for history insertion, clear/replay, cursor, suspend;
- picker and prompt tests for focus/cancel/confirm behavior.

Manual terminal matrix:

- macOS Terminal.app;
- iTerm2;
- tmux;
- Zellij;
- Linux terminal;
- SSH session;
- focus regain;
- Ctrl-Z / `fg`;
- long tool output;
- `/clear`, rewind/truncate, session switch.
- screen-reader smoke checks with macOS VoiceOver and NVDA through tmux/SSH:
  finalized history should be announced as ordinary terminal output.

Observability:

- counter `tui_history_replay_count`;
- counter `tui_history_replay_truncated_count`;
- counter `tui_history_replay_failed_count`;
- counter `tui_stream_consolidation_repair_count`;
- counter `tui_effect_cancelled_count`;
- gauge/counter `tui_zellij_compat_mode_active`;
- span `tui.draw` with `surface_mode`, `width`, `height`,
  `history_rows_emitted`, `replay_reason`, and `overlay_active` fields, following
  the naming discipline in `common/otel/CLAUDE.md`.

## Final Delivery Units

The final console is delivered as one coherent system. The units below are not
sequential milestones; they are the architectural parts that must exist together
for the final agent console to be considered complete.

| Unit | Required final state |
|---|---|
| State and update | `AppState` remains canonical; update reducers mutate state and emit typed effects; render code does not perform side effects. |
| Presentation | Every complex surface consumes typed view models and semantic styles; render measurement is width-aware and testable. |
| Terminal surface | `SurfaceTerminal` owns native scrollback, viewport geometry, cursor style, replay, resize, suspend/resume, and alt-screen pager placement. |
| Transcript | Finalized `TranscriptCell`s are source-backed, replayable, copy-friendly, and emitted to terminal history exactly once per generation. |
| Streaming | Active streams use source-backed stable/tail partitioning with table holdback and resize repair. |
| Activity | `TurnActivityView` unifies running tools, subagents, hooks, tasks, plans/todos, queue state, and stream health; completed work commits into transcript cells. |
| Bottom pane | Composer, attachments, paste burst, suggestions, queued input, local prompts, and footer state are retained and locally routed. |
| Prompts | Permission, question, MCP elicitation, plan, cost, trust, and sandbox prompts use typed view models, explicit priority, explicit placement, and transcript-visible outcomes. |
| Pickers | Model, command, file, skill, session, memory, MCP, keybinding, and settings surfaces share dense picker behavior and width-aware layouts. |
| Pager/review | Transcript, diff, plan, status detail, help, and large search/review surfaces use shared pager/renderable infrastructure. |
| Peripherals | Clipboard, external editor, terminal title, desktop notifications, focus, bracketed paste, and keyboard modes are typed effects with user-visible results when relevant. |
| Verification | Reducer tests, presentation snapshots, stream tests, transcript cell tests, VT100 tests, and terminal-matrix checks cover the relevant final surfaces. |

## Design Verdict

The TEA / `AppState` approach is viable for a full codex-class agent console and
is preferable for `coco-rs`. It gives better provider separation and testability
than copying `codex-rs/tui`'s large widget/controller ownership.

The required additions are:

- typed transcript cells;
- source-backed streaming;
- bottom-pane local surface stack;
- shared picker/pager/prompt scaffolds;
- unified live activity view;
- coco-owned terminal surface.

With those additions, `coco-rs` can reach feature parity in agent-console
capability while keeping its architecture aligned with the rest of the
workspace.
