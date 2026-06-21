use serde::Deserialize;
use serde::Serialize;

use crate::TokenUsage;
use crate::wire_tagged::wire_tagged_enum;

/// Three-layer event envelope.
///
/// All consumers (TUI, SDK, CLI, App-Server) receive `CoreEvent` via
/// `mpsc::channel`. Each consumer matches on the layer it cares about:
///
/// - **TUI**: all 3 layers (exhaustive match, no intermediate bridge type)
/// - **SDK/CLI**: Protocol + Stream (via `StreamAccumulator`; TuiEvent dropped)
/// - **App-Server**: Protocol + Stream (TuiEvent dropped)
///
/// # Ordering contract
///
/// `mpsc` provides FIFO ordering **per sender**. When multiple tasks clone
/// the same `Sender<CoreEvent>` and emit concurrently, cross-sender
/// ordering is **not guaranteed**.
///
/// Where ordering matters, all related events must be emitted from a
/// single task. Current ownership (one sequence = one task):
///
/// - **Turn lifecycle** (`TurnStarted → Stream* → TurnEnded`):
///   `TurnStarted` is emitted **exactly once per logical user-prompt
///   cycle** by the runner layer (`tui_runner` / `sdk_runner` /
///   `TurnRunner` impls). The runner generates a fresh `TurnId`
///   (`TurnId::generate()`), emits `TurnStarted` with that id, then
///   threads the id into the engine via `run_with_messages` /
///   `run_with_events`. Every `TurnEnded` for the cycle reuses the
///   same id. Intra-turn LLM rounds (Stop-hook blocking, reactive
///   compact, max-tokens recovery, every `ContinueReason::*`) do
///   NOT re-emit lifecycle events. A second `TurnEnded(Interrupted)`
///   MAY follow a `TurnEnded(Completed|Failed|MaxTurnsReached|
///   BudgetExhausted)` when a user cancel was observed after the
///   engine had already returned — this is the late-cancel signal
///   the TUI uses to fire auto-restore. `TurnEnded.outcome` carries
///   the discriminated terminal reason (`Completed { stop_reason }`
///   / `Failed { error }` / `Interrupted { abort_reason }` /
///   `MaxTurnsReached { max_turns }` / `BudgetExhausted { used_tokens,
///   budget_tokens }`).
/// - **Session lifecycle** (`SessionStarted → (Running ↔ Idle ↔ RequiresAction)*
///   → SessionResult → SessionEnded`): emitted by `run_internal_with_messages`
///   in `coco-query::engine`; `SessionStateChanged` transitions are deduped
///   via `SessionStateTracker` (see `coco-query::session_state`).
/// - **Hook lifecycle** (`HookStarted → HookProgress* → HookResponse`):
///   emitted by the `forward_hook_events` child task in `coco-query::engine`.
///   Cancellation + 5s drain-on-shutdown protect trailing events.
/// - **Task lifecycle** (`TaskStarted → TaskProgress* → TaskCompleted`):
///   emitted by `TaskManager` when built with `with_event_sink(tx)`.
///   One task manager serializes emissions for all managed tasks.
/// - **Item lifecycle** (`ItemStarted → ItemUpdated → ItemCompleted`) and
///   content deltas (`AgentMessageDelta`, `ReasoningDelta`):
///   **SDK path only**. Produced by `StreamAccumulator` inside the SDK
///   dispatcher's writer task (single task, per-turn accumulator). The
///   TUI consumes `AgentStreamEvent` directly and never sees these.
/// - **Wire serialization**: the SDK dispatcher's writer task is the single
///   serializer — all events pass through one `tokio::select!` loop with
///   `biased;` preferring notifications over replies, so wire order matches
///   channel-receive order.
///
/// ## Known cross-sender emission sites (tolerated)
///
/// - `ContextCompacted` is emitted from two sites inside `run_session_loop`
///   (reactive compaction and auto-compaction). Semantics are idempotent;
///   consumers may see two notifications carrying the same summary.
/// - `Error` may be emitted from budget-exhaustion and query-execution
///   paths. Consumers MUST treat Errors as independent signals; they are
///   not sequenced relative to other events.
///
/// See `event-system-design.md` §12 and plan WS-8.
///
/// **`large_enum_variant` exemption.** `Protocol(ServerNotification)` is
/// considerably larger than `Stream` / `Tui` because `ServerNotification`
/// carries 59 wire-tagged variants. Boxing it would churn hundreds of
/// pattern matches across `coco-query` / `coco-tui` / `coco-cli` for a
/// per-event overhead that is dominated by per-turn work. Each
/// `CoreEvent` is sent over `mpsc`, consumed once, and dropped — the
/// stack-size penalty is bounded and short-lived.
///
/// New large variants in `ServerNotification` itself should be Boxed at
/// the variant payload level if they appear — that's where the warning
/// is actionable.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum CoreEvent {
    /// Protocol-level notifications visible to ALL consumers.
    Protocol(ServerNotification),

    /// Agent-loop stream events requiring accumulation before SDK consumption.
    /// TUI consumes directly for real-time display; SDK passes through
    /// `StreamAccumulator` which converts to `Protocol(ItemStarted/Updated/Completed)`.
    Stream(AgentStreamEvent),

    /// TUI-exclusive events (overlays, toasts, streaming deltas for display).
    /// SDK and App-Server consumers DROP these.
    Tui(TuiOnlyEvent),
}

// ---------------------------------------------------------------------------
// AgentStreamEvent — accumulation-layer stream events
// ---------------------------------------------------------------------------

/// Agent-loop stream events. Higher-level than `coco_types::StreamEvent`
/// (which represents raw LLM inference deltas). Adds:
/// - Tool lifecycle states (Queued → Started → Completed)
/// - MCP tool call tracking
/// - Turn-scoped item IDs
///
/// Input to `StreamAccumulator`.
/// See `event-system-design.md` Section 1.5.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    /// Text content delta from assistant response.
    TextDelta { turn_id: String, delta: String },
    /// Thinking/reasoning delta from extended thinking.
    ThinkingDelta { turn_id: String, delta: String },
    /// Tool use block received from API (input complete). Creates a ThreadItem.
    ToolUseQueued {
        call_id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution has begun (after permission check).
    ToolUseStarted {
        call_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<String>,
    },
    /// Tool execution completed with result.
    ///
    /// `name` is carried here so StreamAccumulator and TUI consumers can
    /// reconstruct display state without maintaining their own call_id → name map.
    ToolUseCompleted {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    /// MCP tool call initiated (separate from builtin tools).
    McpToolCallBegin {
        server: String,
        tool: String,
        call_id: String,
    },
    /// MCP tool call completed.
    McpToolCallEnd {
        server: String,
        tool: String,
        call_id: String,
        is_error: bool,
    },
}

// ---------------------------------------------------------------------------
// ThreadItem — semantic conversation thread items
// ---------------------------------------------------------------------------

/// Semantic representation of a conversation thread item.
/// Produced by `StreamAccumulator` from `AgentStreamEvent` sequences.
/// Used in `ServerNotification::ItemStarted / ItemUpdated / ItemCompleted`.
///
/// See `event-system-design.md` Section 1.6.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadItem {
    pub item_id: String,
    pub turn_id: String,
    pub details: ThreadItemDetails,
}

/// Tool-specific semantic mapping.
///
/// Mapping rules (from `event-system-design.md` Section 6.2):
/// - Bash → `CommandExecution`
/// - Edit/Write → `FileChange`
/// - WebSearch → `WebSearch`
/// - mcp__* → `McpToolCall`
/// - Agent/Task → `Subagent`
/// - all others → `ToolCall`
/// - text content → `AgentMessage`
/// - thinking → `Reasoning`
/// - errors → `Error`
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadItemDetails {
    /// Bash tool → command execution with output.
    CommandExecution {
        command: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        status: ItemStatus,
    },
    /// Edit/Write tools → file change with diff info.
    FileChange {
        changes: Vec<FileChangeInfo>,
        status: ItemStatus,
    },
    /// WebSearch tool.
    WebSearch { query: String, status: ItemStatus },
    /// MCP server tool call.
    McpToolCall {
        server: String,
        tool: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        status: ItemStatus,
    },
    /// Agent/Task tool → subagent lifecycle.
    Subagent {
        agent_id: String,
        agent_type: String,
        description: String,
        #[serde(default)]
        is_background: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        status: ItemStatus,
    },
    /// All other tools (Read, Glob, Grep, etc.).
    ToolCall {
        tool: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default)]
        is_error: bool,
        status: ItemStatus,
    },
    /// Assistant text content.
    AgentMessage { text: String },
    /// Reasoning/thinking content.
    Reasoning { text: String },
    /// Error during processing.
    Error { message: String },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeInfo {
    pub path: String,
    pub kind: FileChangeKind,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Create,
    Modify,
    Delete,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

/// Per-row metadata for a single rewind picker row.
/// `metadata == None` corresponds to `canRestore = false` (no
/// snapshot — picker shows "⚠ No code restore").
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindRowMetadata {
    pub message_id: String,
    pub metadata: Option<RewindDiffStatsPayload>,
}

/// Diff stats payload shared by per-row metadata and the selected
/// restore preview event. `file_paths.is_empty()` means "snapshot
/// exists but nothing changed".
///
/// `file_paths` matches `DiffStats.filesChanged: string[]` and is used by the
/// confirm screen to assemble single / two / many-file labels.
///
/// # Two semantics, one wire type
///
/// `insertions` / `deletions` interpretation depends on which event
/// carries this payload:
///
/// - [`TuiOnlyEvent::RewindRowMetadataReady`] — **forward-time**.
///   `insertions` = lines added between two adjacent user-message
///   checkpoints. Computed via [`FileHistoryState::get_diff_stats_between`]
///   to mirror TS `computeDiffStatsBetweenMessages`
///   (`MessageSelector.tsx:722-765`), which counts `+` lines from
///   `structuredPatch`.
/// - [`TuiOnlyEvent::RewindRestorePreviewReady`] — **rewind-direction**.
///   `insertions` = lines that rewind would add back; `deletions` =
///   lines that rewind would remove. Computed via
///   [`FileHistoryState::get_diff_stats`] to mirror TS
///   `computeDiffStatsForFile` (`fileHistory.ts:705`)'s
///   `diffLines(originalContent, backupContent)` direction.
///
/// TS itself returns the same `DiffStats` shape from both call sites
/// and lets context disambiguate — Rust matches that contract.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RewindDiffStatsPayload {
    pub insertions: i64,
    pub deletions: i64,
    pub file_paths: Vec<String>,
}

impl RewindDiffStatsPayload {
    /// Number of files in `file_paths`. Single source of truth — derived
    /// rather than stored so the count cannot drift from the array.
    pub fn files_changed(&self) -> usize {
        self.file_paths.len()
    }
}

// ---------------------------------------------------------------------------
// NotificationMethod + ServerNotification — protocol-layer notifications
// ---------------------------------------------------------------------------

wire_tagged_enum! {
    method_enum = NotificationMethod,
    tagged_enum = ServerNotification,
    method_doc = "\
Wire-method identifier for every `ServerNotification` variant.\n\n\
Cross-language protocol constant exported to the JSON schema bundle so \
Python / other SDK codegens obtain the same vocabulary. Consumers should \
reference `NotificationMethod::SessionStarted` rather than compare against \
raw wire strings.",
    tagged_doc = "\
Protocol-level notifications visible to all consumers.\n\n\
Protocol notifications across 20 categories. Subagent lifecycle (spawn / progress / \
completion / background transition) rides on `task/started`, `task/progress`, \
and `task/completed` with `task_type` discriminating (`local_agent` / \
`in_process_teammate`), matching TS — no dedicated `subagent/*` family. \
See `event-system-design.md` Section 2 and \
`engine-tui-unified-transcript-plan.md` §4.1 for the history lifecycle \
category. Each variant's wire method is generated together with the \
matching `NotificationMethod` discriminant.",
    variants = {
    // === Session lifecycle (3) ===

    /// New session started.
    "session/started" => SessionStarted(SessionStartedParams),
    /// Session result (final usage, cost, stop reason).
    "session/result" => SessionResult(Box<SessionResultParams>),
    /// Session ended.
    "session/ended" => SessionEnded(SessionEndedParams),
    /// Session usage snapshot updated.
    "session/usageUpdated" => SessionUsageUpdated(Box<crate::SessionUsageSnapshot>),

    // === History lifecycle (4) ===
    //
    // Engine MessageHistory is single source of truth. These events let
    // TUI / SDK consumers maintain derived views without recomputing
    // engine-side state. The Message body is carried typed: coco-types
    // now owns the Message family (relocated from coco-messages) and
    // reaches vercel-ai DTOs through coco-llm-types, so the wire enum
    // can name `Message` directly without bridging through Value.
    //
    // See `engine-tui-unified-transcript-plan.md` §4.1.

    /// One Message appended to engine MessageHistory.
    ///
    /// `session_id` + `agent_id` envelope (plan §11 F9): merged-timeline
    /// consumers (AgentTeams) demux per session/agent off the same event
    /// stream. `agent_id` is `None` for the main agent; `Some` for
    /// teammates / subagents. Single-session SDK consumers may ignore
    /// both fields (`#[serde(default)]` keeps the wire forward-compat).
    "history/messageAppended" => MessageAppended {
        message: std::sync::Arc<crate::messages::Message>,
        #[serde(default)]
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// MessageHistory truncated to `keep_count` entries (indices
    /// >= keep_count discarded). Emitted by explicit-rewind and
    /// auto-restore both, so SDK + TUI converge on engine truncation
    /// without separate private paths.
    "history/messageTruncated" => MessageTruncated {
        keep_count: i64,
        #[serde(default)]
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Session reset for resume. TUI clears derived transcript view
    /// in preparation for a burst of `MessageAppended` that replays
    /// the loaded JSONL transcript.
    "history/resetForResume" => SessionResetForResume {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Bulk snapshot for resume hydration. Consumers replace the
    /// derived transcript view wholesale (one cache-rebuild pass)
    /// instead of processing N `MessageAppended` events sequentially.
    /// Used when loading large JSONL transcripts where the
    /// per-message channel-bounded path would stall at the 256-msg
    /// queue boundary and force the engine task to yield. Live
    /// appends still use `MessageAppended` — this variant models
    /// bulk replacement (a genuinely different operation).
    "history/replaced" => HistoryReplaced {
        messages: Vec<std::sync::Arc<crate::messages::Message>>,
        #[serde(default)]
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Reasoning aggregates attached to a specific assistant message.
    /// Engine emits this after `TurnCompleted` (when usage is known)
    /// so the TUI side-cache anchors `Thinking · <duration> · <tokens>`
    /// by the message UUID rather than re-walking transcript cells.
    /// Eliminates the prior I-2 exception in `TranscriptView`.
    "history/reasoningMetadataAttached" => ReasoningMetadataAttached(ReasoningMetadataAttachedParams),

    // === Turn lifecycle (2) ===

    /// Agent turn started. Emitted by the runner layer
    /// (`tui_runner` / `sdk_runner`) at the start of every logical
    /// user-prompt cycle. Pairs 1:1 with `TurnEnded` sharing the
    /// same `turn_id`. Intra-turn loops (Stop-hook blocking,
    /// reactive compact, max-tokens recovery, etc. — every
    /// `ContinueReason::*`) do NOT re-emit this event.
    "turn/started" => TurnStarted(TurnStartedParams),
    /// Agent turn ended. Discriminated by `outcome.kind` —
    /// `completed` / `failed` / `interrupted` / `max_turns_reached` /
    /// `budget_exhausted`. Paired 1:1 with the preceding
    /// `TurnStarted` (same `turn_id`). See `TurnOutcome` for
    /// variant payloads; `stop_reason` only appears on `Completed`
    /// where it is semantically meaningful.
    "turn/ended" => TurnEnded(TurnEndedParams),

    // === Item lifecycle (3) ===

    /// Thread item started (from StreamAccumulator).
    "item/started" => ItemStarted { item: ThreadItem },
    /// Thread item updated (e.g. tool execution began).
    "item/updated" => ItemUpdated { item: ThreadItem },
    /// Thread item completed.
    "item/completed" => ItemCompleted { item: ThreadItem },

    // === Content deltas (2) ===

    /// Text content delta from assistant.
    "agentMessage/delta" => AgentMessageDelta(ContentDeltaParams),
    /// Reasoning/thinking delta.
    "reasoning/delta" => ReasoningDelta(ContentDeltaParams),

    // === MCP (2) ===

    /// MCP server startup status.
    "mcp/startupStatus" => McpStartupStatus(McpStartupStatusParams),
    /// All MCP servers finished startup.
    "mcp/startupComplete" => McpStartupComplete(McpStartupCompleteParams),

    // === LSP (1) ===

    /// LSP server pool finished prewarm. Fired once per session
    /// bootstrap (after `LspManagerAdapter::prewarm` completes), so the
    /// TUI status bar can show a `LSP` badge with the running-server
    /// count. Not emitted when `Feature::Lsp` is off — `started` /
    /// `failed` are empty in that case.
    "lsp/prewarmComplete" => LspPrewarmComplete(LspPrewarmCompleteParams),

    // === Context (6) ===

    /// Context compacted.
    "context/compacted" => ContextCompacted(ContextCompactedParams),
    /// Context usage warning.
    "context/usageWarning" => ContextUsageWarning(ContextUsageWarningParams),
    /// Compaction started.
    "context/compactionStarted" => CompactionStarted,
    /// Compaction phase progress (TS `onCompactProgress`).
    /// Drives the spinner text in the TUI / SDK runner so the user
    /// can see which sub-phase is active (PreCompact hooks → summarize
    /// → PostCompact hooks → done).
    "context/compactionPhase" => CompactionPhase(CompactionPhaseParams),
    /// Compaction failed.
    "context/compactionFailed" => CompactionFailed(CompactionFailedParams),
    /// Context cleared (e.g. new mode).
    "context/cleared" => ContextCleared(ContextClearedParams),

    // === Task (6) ===

    /// Background task started.
    "task/started" => TaskStarted(TaskStartedParams),
    /// Background task completed.
    "task/completed" => TaskCompleted(TaskCompletedParams),
    /// Background task progress.
    "task/progress" => TaskProgress(TaskProgressParams),
    /// Durable plan-item / V1 todo snapshot — emitted after
    /// `TaskCreate`/`TaskUpdate`/`TodoWrite` tools mutate state so
    /// the TUI can refresh its panel without pulling the store
    /// directly. TS parity: `notifyTasksUpdated` subscriber callback
    /// in `utils/tasks.ts`.
    "task_panel/changed" => TaskPanelChanged(TaskPanelChangedParams),
    /// Team lead received a plan-approval request from a teammate
    /// (via mailbox). The TUI surfaces this as a modal overlay.
    /// TS parity: `ExitPlanModeV2Tool.ts:137-141` teammate request flow.
    "plan_approval/requested" => PlanApprovalRequested(PlanApprovalRequestedParams),
    /// Agents killed.
    "agents/killed" => AgentsKilled(AgentsKilledParams),

    // === Model (4) ===

    /// Model fallback started.
    "model/fallbackStarted" => ModelFallbackStarted(ModelFallbackParams),
    /// Model fallback completed.
    "model/fallbackCompleted" => ModelFallbackCompleted,
    /// Fast mode state changed.
    "model/fastModeChanged" => FastModeChanged { active: bool },
    /// A role's binding (model + provider + effort) changed in-memory
    /// via the picker or `Ctrl+T`. Carries the resolved fields the TUI
    /// needs to refresh its `model_by_role` cache and, for `Main`,
    /// status-bar fields (`model`, `provider`, `thinking_effort`).
    "model/roleChanged" => ModelRoleChanged(ModelRoleChangedParams),

    // === Permission (1) ===

    /// Permission mode changed.
    "permission/modeChanged" => PermissionModeChanged(PermissionModeChangedParams),

    // === Prompt (1) ===

    /// Prompt suggestions.
    "prompt/suggestion" => PromptSuggestion { suggestions: Vec<String> },

    // === System (3) ===

    /// Error notification.
    "error" => Error(ErrorParams),
    /// Rate limit notification.
    "rateLimit" => RateLimit(RateLimitParams),
    /// Keep-alive heartbeat.
    "keepAlive" => KeepAlive { timestamp: i64 },

    // === IDE (2) ===

    /// IDE selection changed.
    "ide/selectionChanged" => IdeSelectionChanged(IdeSelectionChangedParams),
    /// IDE diagnostics updated.
    "ide/diagnosticsUpdated" => IdeDiagnosticsUpdated(IdeDiagnosticsUpdatedParams),

    // === Queue (3) ===

    /// Command queue state changed.
    "queue/stateChanged" => QueueStateChanged { queued: i32 },
    /// Command queued.
    "queue/commandQueued" => CommandQueued { id: String, preview: String, editable: bool },
    /// Command dequeued.
    "queue/commandDequeued" => CommandDequeued { id: String },

    // === Rewind (2) ===

    /// File rewind completed.
    "rewind/completed" => RewindCompleted(RewindCompletedParams),
    /// File rewind failed.
    "rewind/failed" => RewindFailed { error: String },

    // === Cost (1) ===

    /// Cost threshold warning.
    "cost/warning" => CostWarning(CostWarningParams),

    // === Sandbox (2) ===

    /// Sandbox state changed.
    "sandbox/stateChanged" => SandboxStateChanged(SandboxStateChangedParams),
    /// Sandbox violations detected.
    "sandbox/violationsDetected" => SandboxViolationsDetected { count: i32 },

    // === Agent (1) ===

    /// Agents registered.
    "agents/registered" => AgentsRegistered { agents: Vec<AgentInfo> },

    // === Hook (3 — TS lifecycle trio) ===

    /// Hook execution started.
    "hook/started" => HookStarted(HookStartedParams),
    /// Hook execution progress (TS gap P1 — stdout/stderr streaming).
    "hook/progress" => HookProgress(HookProgressParams),
    /// Hook execution completed (TS gap P1).
    "hook/response" => HookResponse(HookResponseParams),

    // === Worktree (2) ===

    /// Entered a worktree.
    "worktree/entered" => WorktreeEntered(WorktreeEnteredParams),
    /// Exited a worktree.
    "worktree/exited" => WorktreeExited(WorktreeExitedParams),

    // === Summarize (2) ===

    /// Summarization completed.
    "summarize/completed" => SummarizeCompleted(SummarizeCompletedParams),
    /// Summarization failed.
    "summarize/failed" => SummarizeFailed { error: String },

    // === Stream health (3) ===

    /// Stream stall detected.
    "stream/stallDetected" => StreamStallDetected {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    /// Stream watchdog warning.
    "stream/watchdogWarning" => StreamWatchdogWarning { elapsed_secs: f64 },
    /// Stream request ended (with usage).
    "stream/requestEnd" => StreamRequestEnd { usage: TokenUsage },

    // === TS Gap P1: Session state (1) ===

    /// Session state changed (idle/running/requires_action).
    "session/stateChanged" => SessionStateChanged { state: SessionState },

    // === TS gap P2: additional SDK notifications (5) ===

    /// Output from a user-executed local command (REPL `!` prefix).
    /// Matches TS `SDKLocalCommandOutputMessage` (coreSchemas.ts:1590-1602).
    "localCommand/output" => LocalCommandOutput(LocalCommandOutputParams),
    /// Files persisted to disk (file upload/snapshot completion).
    /// Matches TS `SDKFilesPersistedEvent` (coreSchemas.ts:1672-1692).
    "files/persisted" => FilesPersisted(FilesPersistedParams),
    /// MCP elicitation completed (form submission or cancellation).
    /// Matches TS `SDKElicitationCompleteMessage` (coreSchemas.ts:1779-1792).
    "elicitation/complete" => ElicitationComplete(ElicitationCompleteParams),
    /// Tool use summary from background haiku summarization.
    /// Matches TS `SDKToolUseSummaryMessage` (coreSchemas.ts:1769-1777).
    "tool/useSummary" => ToolUseSummary(ToolUseSummaryParams),
    /// Tool execution progress (bash/powershell long-running).
    /// Matches TS `SDKToolProgressMessage` (coreSchemas.ts:1648-1659).
    /// Sent at most once per 30 seconds per `parent_tool_use_id`.
    "tool/progress" => ToolProgress(ToolProgressParams),

    // === Plugins (1) ===

    /// Plugin state changed on disk (manifest added/removed/edited,
    /// `installed_plugins.json` updated, or settings.json scope toggled).
    /// Carries a short reason string the UI can surface as a banner.
    /// TS parity: `useManagePlugins.ts:293-300` adds the "Plugins
    /// changed. Run /reload-plugins to activate." notification. Never
    /// triggers an auto-reload — the explicit `/reload-plugins`
    /// invocation is what applies the change.
    "plugins/changed" => PluginsChanged { reason: String },
    }
}

// Compile-time regression guard: keep `ServerNotification` from growing
// unbounded. The enum's size is the size of the largest variant; every
// `CoreEvent` pays this cost (inlined in mpsc channel buffers). If a new
// variant pushes this past the limit, either `Box<T>` the offending params
// (like `SessionResult(Box<SessionResultParams>)`) or justify raising the
// limit. Don't let it drift silently.
const _: () = assert!(
    std::mem::size_of::<ServerNotification>() <= 400,
    "ServerNotification exceeded 400 bytes; Box<T> the largest variant"
);

// ---------------------------------------------------------------------------
// ServerNotification param structs
// ---------------------------------------------------------------------------

/// Matches TS `SDKSystemMessageSchema` with subtype 'init' (coreSchemas.ts:1457-1494).
/// Sent once at session startup; carries the full bootstrap context the SDK
/// consumer needs to render a UI.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartedParams {
    pub session_id: String,
    /// coco-rs extension: protocol version negotiation.
    pub protocol_version: String,
    pub cwd: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub permission_mode: String,
    /// Builtin + MCP tool names.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Slash commands available in this session.
    #[serde(default)]
    pub slash_commands: Vec<String>,
    /// Agent type names available for Agent tool spawning.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Skill names loaded.
    #[serde(default)]
    pub skills: Vec<String>,
    /// MCP server status at initialization.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerInit>,
    /// Loaded plugin metadata.
    #[serde(default)]
    pub plugins: Vec<PluginInit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    /// Release version of the coco-rs binary.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode_state: Option<FastModeState>,
    /// `true` when at least one LSP server is healthy at session
    /// startup (`Feature::Lsp` on + adapter `is_connected() = true`).
    /// Used by the TUI status bar to render an "LSP" badge. Default
    /// `false` keeps legacy SDK clients backward-compatible.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lsp_active: bool,
}

/// MCP server init entry (inline struct in TS).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInit {
    pub name: String,
    pub status: crate::server_request::McpConnectionStatus,
}

/// Plugin init entry (inline struct in TS).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInit {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKResultMessageSchema` (coreSchemas.ts:1407-1451).
/// TS has two subtype variants (success/error) unified here with `is_error` flag.
pub struct SessionResultParams {
    pub session_id: String,
    pub total_turns: i32,
    pub duration_ms: i64,
    pub duration_api_ms: i64,
    #[serde(default)]
    pub is_error: bool,
    pub stop_reason: String,
    pub total_cost_usd: f64,
    pub usage: TokenUsage,
    /// Per-model usage breakdown (TS `modelUsage: Record<string, ModelUsage>`).
    #[serde(default)]
    pub model_usage: std::collections::HashMap<String, SessionModelUsage>,
    /// Permission denials accumulated during the session.
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenialInfo>,
    /// Success variant: the agent's final result text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error variant: list of error strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode_state: Option<FastModeState>,
    /// coco-rs extension: num_api_calls for observability (not in TS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_api_calls: Option<i32>,
}

/// Matches TS `ModelUsageSchema` (coreSchemas.ts:17-28).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionModelUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub web_search_requests: i64,
    pub cost_usd: f64,
    pub context_window: i64,
    pub max_output_tokens: i64,
}

/// Matches TS `SDKPermissionDenialSchema` (coreSchemas.ts:1399-1405).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDenialInfo {
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: serde_json::Value,
}

/// Matches TS `FastModeStateSchema` (coreSchemas.ts:1883-1889).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastModeState {
    Off,
    Cooldown,
    On,
}

// ---------------------------------------------------------------------------
// TS gap P2: additional SDK notification params
// ---------------------------------------------------------------------------

/// Matches TS `SDKLocalCommandOutputMessage` (coreSchemas.ts:1590-1602).
///
/// TS emits this when the user runs a local bash command via the REPL `!`
/// prefix (not a tool call). The `content` field is the command output;
/// TS types it as the raw output structure (typically stdout/stderr).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalCommandOutputParams {
    pub content: serde_json::Value,
}

/// Matches TS `SDKFilesPersistedEvent` (coreSchemas.ts:1672-1692).
///
/// TS emits this when files are uploaded or persisted (e.g. after a
/// successful `filesApi` operation).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesPersistedParams {
    pub files: Vec<PersistedFileInfo>,
    #[serde(default)]
    pub failed: Vec<PersistedFileError>,
    pub processed_at: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedFileInfo {
    pub filename: String,
    pub file_id: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedFileError {
    pub filename: String,
    pub error: String,
}

/// Matches TS `SDKElicitationCompleteMessage` (coreSchemas.ts:1779-1792).
///
/// Emitted after an MCP server's elicitation request is resolved
/// (either submitted or cancelled).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCompleteParams {
    pub mcp_server_name: String,
    pub elicitation_id: String,
}

/// Matches TS `SDKToolUseSummaryMessage` (coreSchemas.ts:1769-1777).
///
/// Background Haiku-based summary of a batch of tool uses. TS uses this
/// to compress verbose tool output before it's displayed or archived.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseSummaryParams {
    pub summary: String,
    pub preceding_tool_use_ids: Vec<String>,
}

/// Matches TS `SDKToolProgressMessage` (coreSchemas.ts:1648-1659).
///
/// Long-running tool progress (Bash, PowerShell). TS throttles emission to
/// ≤1 per 30 seconds per `parent_tool_use_id`. coco-rs StreamAccumulator
/// may emit this independently from `AgentStreamEvent::ToolUseStarted`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgressParams {
    pub tool_use_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    pub elapsed_time_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndedParams {
    pub reason: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartedParams {
    pub turn_id: crate::TurnId,
}

/// Terminal event for one logical user-prompt cycle. The
/// `outcome` discriminator carries the per-variant payload —
/// `stop_reason` lives only on `Completed` where it is
/// semantically meaningful. Other variants self-describe.
///
/// Pairing contract: every `TurnStartedParams` is followed by
/// **at least one** `TurnEndedParams` sharing the same `turn_id`.
/// A second `TurnEnded(Interrupted)` MAY follow a terminal outcome
/// when the user cancel was observed after the engine had already
/// returned — this is the late-cancel signal the TUI uses to fire
/// auto-restore. Consumers should treat the latest `Interrupted`
/// event as authoritative when it follows a same-id terminal.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnEndedParams {
    pub turn_id: crate::TurnId,
    /// Token usage accumulated over the cycle. `None` only when
    /// the emitter genuinely does not know (runner-side bail
    /// before engine entry, or late-cancel emitter that does not
    /// own the engine's accumulator). `Some(TokenUsage::default())`
    /// is a real "zero usage" signal — distinct from "unknown".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    pub outcome: TurnOutcome,
}

/// Discriminated terminal reason. The `Completed` variant is
/// the only one that carries `stop_reason`: the other variants'
/// names ARE the terminal reason.
///
/// Wire format: `{"kind": "completed", "data": {"stop_reason": "end_turn"}}` etc.
///
/// Per-variant payloads are **named structs** rather than inline
/// struct-variants. This is deliberate: schemars emits `$ref` to the
/// named struct, which the Python codegen turns into a real Pydantic
/// model. Inline struct-variants would degrade to `dict[str, Any]` on
/// the Python side — losing typed access for SDK consumers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// Engine decided to terminate the cycle.
    Completed(CompletedOutcome),
    /// Engine error: provider failure, network, internal panic,
    /// config invalid, etc.
    Failed(FailedOutcome),
    /// External cancellation (user Ctrl+C, submit interrupt, permission abort,
    /// system pre-empt).
    Interrupted(InterruptedOutcome),
    /// Turn budget exhausted — the cycle hit `config.max_turns`
    /// before the model issued a non-tool-use stop.
    MaxTurnsReached(MaxTurnsReachedOutcome),
    /// Token budget exhausted (90% threshold / diminishing-returns
    /// stop, distinct from `max_turns`).
    BudgetExhausted(BudgetExhaustedOutcome),
}

/// Payload for [`TurnOutcome::Completed`]. `stop_reason` is the *last
/// LLM round's* finish reason at the moment the engine returned.
/// `None` when the cycle ended without ever resolving a real model
/// finish reason (structured-output retry cap, Stop-hook prevent
/// before any round resolved one) — emitters must not fabricate.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<crate::StopReason>,
}

/// Payload for [`TurnOutcome::Failed`]. `error.code` is the
/// wire-stable category; `error.message` is the human-readable
/// detail. No `stop_reason` — the failure did not originate from
/// the model's finish_reason.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedOutcome {
    pub error: ErrorPayload,
}

/// Payload for [`TurnOutcome::Interrupted`]. `abort_reason`
/// distinguishes user Ctrl+C, submit interrupt, permission abort, and
/// system pre-empt. Never carries a model `stop_reason`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptedOutcome {
    pub abort_reason: TurnAbortReason,
}

/// Payload for [`TurnOutcome::MaxTurnsReached`]. The variant IS the
/// terminal reason; consumers know "you ran out of turns".
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaxTurnsReachedOutcome {
    pub max_turns: i32,
}

/// Payload for [`TurnOutcome::BudgetExhausted`]. `budget_tokens` is
/// the configured `max_tokens` ceiling; `None` when no explicit max
/// was set (the 90%-of-window heuristic still drove the stop, but
/// there is no honest single number to emit).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetExhaustedOutcome {
    pub used_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i64>,
}

impl TurnEndedParams {
    pub fn completed(
        turn_id: crate::TurnId,
        usage: Option<TokenUsage>,
        stop_reason: Option<crate::StopReason>,
    ) -> Self {
        Self {
            turn_id,
            usage,
            outcome: TurnOutcome::Completed(CompletedOutcome { stop_reason }),
        }
    }

    pub fn failed(turn_id: crate::TurnId, usage: Option<TokenUsage>, error: ErrorPayload) -> Self {
        Self {
            turn_id,
            usage,
            outcome: TurnOutcome::Failed(FailedOutcome { error }),
        }
    }

    pub fn interrupted(
        turn_id: crate::TurnId,
        usage: Option<TokenUsage>,
        abort_reason: TurnAbortReason,
    ) -> Self {
        Self {
            turn_id,
            usage,
            outcome: TurnOutcome::Interrupted(InterruptedOutcome { abort_reason }),
        }
    }

    pub fn max_turns_reached(
        turn_id: crate::TurnId,
        usage: Option<TokenUsage>,
        max_turns: i32,
    ) -> Self {
        Self {
            turn_id,
            usage,
            outcome: TurnOutcome::MaxTurnsReached(MaxTurnsReachedOutcome { max_turns }),
        }
    }

    pub fn budget_exhausted(
        turn_id: crate::TurnId,
        usage: Option<TokenUsage>,
        used_tokens: i64,
        budget_tokens: Option<i64>,
    ) -> Self {
        Self {
            turn_id,
            usage,
            outcome: TurnOutcome::BudgetExhausted(BudgetExhaustedOutcome {
                used_tokens,
                budget_tokens,
            }),
        }
    }
}

/// Structured error payload for `TurnOutcome::Failed`. Replaces
/// the older opaque `error: String` so Hub / SDK consumers can
/// filter / aggregate by category without parsing the message.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// Human-readable description. Always present.
    pub message: String,
    /// Wire-stable category. Maps 1:1 from
    /// `coco_error::StatusCategory` (minus `Success`).
    pub code: ErrorCode,
}

/// Wire-stable error category. Category-level (11 variants) rather
/// than leaf `StatusCode` (~35 variants) so the protocol does not
/// break every time a new internal status is added. The mapping
/// from `coco_error::StatusCategory` lives in `coco_query::error_code`
/// (the seam between the error layer and the wire types).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Common,
    Input,
    Io,
    Network,
    Auth,
    Config,
    Provider,
    Resource,
    SystemReminder,
    /// Hook policy blocked the turn (e.g. a `UserPromptSubmit` hook
    /// returned a `blocking_error`). Not a coco_error condition —
    /// it's a deliberate policy decision surfaced via the same wire
    /// shape so consumers can render / filter it. Distinct from
    /// `Unknown` so dashboards don't lump policy blocks together with
    /// the defensive sentinel.
    HookBlocked,
    /// Defensive sentinel: an emitter reached the error path without
    /// a classifiable `StatusCategory` (e.g. the `Success → Unknown`
    /// collapse in `coco_query::error_code`). Treat as a bug signal,
    /// not a load-bearing category — every `Unknown` in production
    /// traffic warrants investigation.
    Unknown,
}

/// Reasoning aggregates anchored to a specific assistant message.
///
/// Emitted by the engine right after `TurnCompleted` when the model
/// reported non-zero `reasoning_tokens`. The TUI handler indexes its
/// `reasoning_metadata` side-cache by `message_uuid`, eliminating the
/// O(n) "find latest `AssistantThinking` cell" walk and removing the
/// last vestige of the I-2 exception.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningMetadataAttachedParams {
    /// Assistant message UUID this metadata anchors to. String-form
    /// for wire stability; `Uuid::parse_str` on the receiving side.
    pub message_uuid: String,
    /// Token-only reasoning duration in milliseconds. `None` when the
    /// provider didn't separate reasoning vs. content time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Cumulative reasoning tokens for the turn.
    pub reasoning_tokens: i64,
}

/// Why a turn was aborted. Lets consumers distinguish user cancel,
/// submit interrupt, permission abort, and system pre-emption.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAbortReason {
    /// User-initiated cancel (Ctrl+C in the TUI, `control/interrupt`
    /// in the SDK). The only reason that may trigger auto-restore.
    UserCancel,
    /// Streaming submit interruption: the user submitted new input while
    /// all running tools were cancel-interruptible.
    SubmitInterrupt,
    /// System pre-empted the in-flight turn so another session-level
    /// operation can run (Clear / Compact / Rewind / Shutdown / new
    /// SubmitInput). Auto-restore is suppressed — the user did not
    /// request a rewind.
    SystemPreempt,
    /// Permission flow aborted the turn instead of returning a normal
    /// model-visible denial.
    PermissionAbort,
    /// Turn moved to the background.
    Background,
}

/// Structured reason for a tool abort notification.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolAbortReasonPayload {
    /// The whole turn was aborted.
    Turn { reason: TurnAbortReason },
    /// The tool itself was aborted independently of the turn.
    SelfAbort { message: String },
    /// A sibling shell tool failed and aborted this parallel tool.
    SiblingError { failed_tool: String },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentDeltaParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub delta: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupStatusParams {
    pub server: String,
    pub status: crate::server_request::McpConnectionStatus,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupCompleteParams {
    pub servers: Vec<String>,
    #[serde(default)]
    pub failed: Vec<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspPrewarmCompleteParams {
    /// IDs of language servers that successfully spawned (e.g.
    /// `["rust-analyzer", "gopls"]`).
    pub started: Vec<String>,
    /// Workspace root the prewarm anchored at.
    pub root: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextCompactedParams {
    pub removed_messages: i32,
    pub summary_tokens: i32,
    /// Which strategy produced this compaction.
    ///
    /// Mirrors the TS `tengu_compact.trigger` field. Older transcripts
    /// without this field default to `Auto`. Defaulted via serde so
    /// off-the-wire payloads from older SDK clients keep parsing.
    #[serde(default = "default_compact_trigger_param")]
    pub trigger: crate::CompactTrigger,
    /// Estimated tokens before compaction (post-strategy LLM input view).
    /// `None` for paths that do not measure (e.g. micro/time-based may
    /// skip when the savings are 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_tokens: Option<i64>,
    /// Estimated tokens after compaction (resulting context size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_tokens: Option<i64>,
}

fn default_compact_trigger_param() -> crate::CompactTrigger {
    crate::CompactTrigger::Auto
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUsageWarningParams {
    pub estimated_tokens: i64,
    pub warning_threshold: i64,
    pub percent_left: f64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionFailedParams {
    pub error: String,
    #[serde(default)]
    pub attempts: i32,
}

/// Sub-phase of a compaction in progress (TS `onCompactProgress`).
///
/// Mirrors the TS phase taxonomy at Tool.ts:150-156:
///   - `HooksStart { hook_type }` for PreCompact / PostCompact / SessionStart
///   - `Summarizing` for the LLM summarizer call
///   - `Done` to clear the spinner
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPhase {
    HooksStart,
    Summarizing,
    Done,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionHookType {
    PreCompact,
    PostCompact,
    SessionStart,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionPhaseParams {
    pub phase: CompactionPhase,
    /// Set when `phase == HooksStart`. None when not applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_type: Option<CompactionHookType>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextClearedParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_mode: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKTaskStartedMessage` (`coreSchemas.ts:1715-1733`)
/// plus optional teammate-metadata fields used when
/// `task_type == "in_process_teammate"`.
///
/// TS encodes both teammate and async-subagent spawn through this same
/// `task_started` SDK event, discriminated by `task_type` — the canonical
/// strings live in `Task.ts:6-13`: `"local_bash"`, `"local_agent"`,
/// `"remote_agent"`, `"in_process_teammate"`, `"local_workflow"`,
/// `"monitor_mcp"`, `"dream"`. The teammate-roster rich metadata that TS
/// stores in `AppState.teamContext.teammates` rides along as the
/// optional fields below so the TUI in coco-rs (no shared store across
/// processes) can construct the same `SubagentInstance { kind:
/// Teammate, ... }` projection on the wire alone.
pub struct TaskStartedParams {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    // ── Teammate-only fields (populated when task_type ==
    //    "in_process_teammate"). Mirror of TS `teamContext.teammates`
    //    sidecar. ────────────────────────────────────────────────────
    /// Bare agent name (e.g. `"researcher"`). The fully-qualified
    /// `name@team` lives in [`Self::task_id`] for teammate rows so the
    /// existing task identity machinery doesn't need a second key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Team this teammate joins. Empty / missing for solo teammates
    /// and for non-teammate tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    /// Color hint (`AgentColorName`) for the UI badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Backend hosting the teammate process — `"in_process"` /
    /// `"tmux_pane"` / `"tmux_window"`. None for non-teammate tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_kind: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKTaskNotificationMessage` (coreSchemas.ts:1694-1713).
/// TS calls this `task/notification`; coco-rs uses `task/completed` as the
/// wire method for brevity, but fields match TS exactly.
pub struct TaskCompletedParams {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub status: TaskCompletionStatus,
    pub output_file: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
}

/// Matches TS `z.enum(['completed', 'failed', 'stopped'])` for task_notification status.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCompletionStatus {
    Completed,
    Failed,
    Stopped,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKTaskProgressMessage` (coreSchemas.ts:1750-1767).
/// In TS, `description` and `usage` are required; other fields optional.
/// The `workflow_progress` field carries the streaming state of local_workflow
/// tasks — a delta batch of workflow state changes.
pub struct TaskProgressParams {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub description: String,
    pub usage: TaskUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Declared subagent type (`Explore`/`Plan`/…), mirrored from the
    /// progress slot so the TUI can correct the `local_agent` fallback
    /// that `TaskStarted` emits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Recent tool activities (cap-5 ring buffer). Coordinator-side
    /// owns push + eviction; the TUI just copies the slice into its
    /// `SubagentInstance` mirror. TS parity:
    /// `tasks/LocalAgentTask/LocalAgentTask.tsx:40`
    /// `MAX_RECENT_ACTIVITIES = 5`. Empty when the task hasn't run a
    /// tool yet or the producer is a legacy code path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_activities: Vec<crate::task::TaskActivity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_progress: Vec<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUsage {
    pub total_tokens: i64,
    /// Input / output split + cache-read (the `↑in ↓out` / cache-hit
    /// dimensions the panel and transcript summary show). `#[serde(default)]`
    /// for back-compat with payloads that only carried `total_tokens`.
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    pub tool_uses: i32,
    pub duration_ms: i64,
    /// Real USD cost for completed subagent runs. `#[serde(default)]`
    /// keeps older transcripts / mid-flight progress (which carry no
    /// cost) deserializing to `0.0`.
    #[serde(default)]
    pub cost_usd: f64,
}

/// A teammate's plan-approval request, surfaced to the team lead's
/// TUI for approve/deny. Payload byte-matches TS
/// `PlanApprovalRequestSchema` — see `tools/ExitPlanModeTool/`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApprovalRequestedParams {
    /// Correlation id carried back in the response envelope.
    pub request_id: String,
    /// Teammate agent name.
    pub from: String,
    /// Optional on-disk plan file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<String>,
    /// Plan text rendered from the teammate's plan file.
    pub plan_content: String,
}

/// Snapshot of the task panel state — tools emit this post-mutation
/// so the TUI can redraw without reaching into `ToolAppState` directly.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPanelChangedParams {
    pub plan_tasks: Vec<crate::TaskRecord>,
    #[serde(default)]
    pub todos_by_agent: std::collections::HashMap<String, Vec<crate::TodoRecord>>,
    pub expanded_view: crate::ExpandedView,
    pub verification_nudge_pending: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsKilledParams {
    pub count: i32,
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFallbackParams {
    pub from_model: String,
    pub to_model: String,
    pub reason: String,
}

/// Payload for [`crate::ServerNotification::ModelRoleChanged`]. Carries
/// the resolved binding (model + provider + thinking effort) that the
/// TUI applies to `state.session.model_by_role[role]` and, when
/// `role == Main`, also to `state.session.{model, provider,
/// thinking_effort}` for the status bar.
///
/// Emitted by `tui_runner` after applying an in-memory override via
/// `SessionRuntime::apply_role_override` / `apply_role_effort`. No
/// persistence to settings.json — that's the user's job.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoleChangedParams {
    pub role: crate::ModelRole,
    pub model_id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// `None` ⇒ engine falls back to the model's
    /// `default_thinking_level`. `Some(_)` ⇒ explicit user choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::ReasoningEffort>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionModeChangedParams {
    pub mode: crate::PermissionMode,
    #[serde(default)]
    pub bypass_available: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorParams {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    // TS gap: enhanced fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RateLimitStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeSelectionChangedParams {
    pub file_path: String,
    pub selected_text: String,
    pub start_line: i32,
    pub end_line: i32,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeDiagnosticsUpdatedParams {
    pub file_path: String,
    pub new_count: i32,
    #[serde(default)]
    pub diagnostics: Vec<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindCompletedParams {
    pub rewound_turn: i32,
    pub restored_files: i32,
    pub messages_removed: i32,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostWarningParams {
    pub current_cost_cents: i64,
    pub threshold_cents: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_cents: Option<i64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxStateChangedParams {
    pub active: bool,
    pub enforcement: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStartedParams {
    pub hook_id: String,
    pub hook_name: String,
    pub hook_event: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKHookProgressMessage` (coreSchemas.ts:1616-1629).
pub struct HookProgressParams {
    pub hook_id: String,
    pub hook_name: String,
    pub hook_event: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub output: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Matches TS `SDKHookResponseMessage` (coreSchemas.ts:1631-1646).
pub struct HookResponseParams {
    pub hook_id: String,
    pub hook_name: String,
    pub hook_event: String,
    pub output: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub outcome: HookOutcomeStatus,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutcomeStatus {
    Success,
    Error,
    Cancelled,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeEnteredParams {
    pub worktree_path: String,
    pub branch: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeExitedParams {
    pub worktree_path: String,
    pub action: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizeCompletedParams {
    pub from_turn: i32,
    pub summary_tokens: i32,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Turn completed, waiting for user input.
    Idle,
    /// Agent is actively processing.
    Running,
    /// Waiting for user action (approval, question, elicitation).
    RequiresAction,
}

// ---------------------------------------------------------------------------
// TuiOnlyEvent — TUI-exclusive events (21 variants)
// ---------------------------------------------------------------------------

/// Bounded, UI-ready permission input display.
///
/// This is separate from the raw tool input because approval UIs should
/// consume sanitized display data while keeping `original_input` only for
/// updated-input response construction and permission-rule derivation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PermissionDisplayInput {
    Command(String),
    Json(String),
    Text(String),
    Empty,
}

impl PermissionDisplayInput {
    pub fn as_display_str(&self) -> &str {
        match self {
            Self::Command(value) | Self::Json(value) | Self::Text(value) => value,
            Self::Empty => "",
        }
    }
}

/// TUI-exclusive events.
///
/// These events are dropped by SDK and App-Server consumers. They drive
/// overlays, toasts, and UI-only state transitions that are not part of the
/// protocol contract.
///
/// Per `event-system-design.md` Section 1.7, the design listed this type as
/// owned by `coco-tui`. Since `CoreEvent::Tui(TuiOnlyEvent)` is part of the
/// envelope enum defined in `coco-types`, the type itself must live in
/// `coco-types` to avoid a cyclic dependency. The semantic contract
/// (TUI-only, never sent to SDK) is preserved via consumer dispatch rules
/// in `StreamAccumulator` and `handle_core_event()`.
///
/// 23 variants (20 from design §4.1 + 3 coco-rs extensions).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiOnlyEvent {
    // === Permission / Question overlays (4) ===
    /// Permission approval overlay needed.
    ///
    /// When `choices` is `Some`, the TUI should render a multi-choice
    /// list (e.g. ExitPlanMode's keep/clear/cancel) instead of the
    /// default yes/no buttons. Picked `value` is echoed back via
    /// `UserCommand::ApprovalResponse.resolution_detail` so the tool's
    /// `execute()` can branch on the choice. TS parity:
    /// `ExitPlanModePermissionRequest.tsx:691-704` option grid.
    ///
    /// `original_input` carries the raw tool input so the TUI can derive
    /// path-scoped read permission updates for classic "always allow"
    /// approvals.
    ApprovalRequired {
        request_id: String,
        tool_name: String,
        description: String,
        display_input: PermissionDisplayInput,
        /// Whether the UI may offer an "always allow" action. False
        /// when managed policy restricts local/session rule changes.
        #[serde(default)]
        show_always_allow: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        choices: Option<Vec<crate::PermissionAskChoice>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<crate::PermissionRequestDetail>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permission_suggestions: Vec<crate::PermissionUpdate>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_input: Option<serde_json::Value>,
        /// Tool execution cwd used to resolve relative paths in
        /// `original_input` for scoped permission grants.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Identity badge of the requesting cross-process teammate, if
        /// any — rendered in the prompt so the leader sees who is asking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_badge: Option<crate::WorkerBadge>,
    },
    /// AskUserQuestion overlay needed. `input` carries the full tool
    /// input dict (the `questions[]` array, etc.) verbatim so the TUI
    /// renders the rich multi-question UI (multiSelect, preview, notes)
    /// matching TS `AskUserQuestionPermissionRequest.tsx`. The TUI
    /// echoes the answer payload back via `UserCommand::ApprovalResponse`
    /// with `updated_input = Some({...input, answers, annotations})`.
    QuestionAsked {
        request_id: String,
        input: serde_json::Value,
    },
    /// MCP elicitation overlay needed.
    ElicitationRequested {
        request_id: String,
        server: String,
        schema: serde_json::Value,
    },
    /// Sandbox approval overlay needed.
    SandboxApprovalRequired {
        request_id: String,
        operation: String,
    },
    /// Lazily-fetched permission risk explanation is ready for the prompt
    /// `request_id`. `explanation` is `None` when the fetch failed or the
    /// explainer is disabled (the panel then shows "unavailable"). Produced
    /// after the user toggles the Ctrl+E explainer panel. TS:
    /// `generatePermissionExplanation` (`permissionExplainer.ts`).
    PermissionExplanationReady {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explanation: Option<crate::PermissionExplanation>,
    },

    // === Picker / data-ready events (5) ===
    /// Plugin picker data loaded.
    PluginDataReady { plugins: Vec<serde_json::Value> },
    /// Output style picker data loaded.
    OutputStylesReady { styles: Vec<String> },
    /// Slash-command catalogue changed — sent after the seed at session
    /// start AND whenever `/reload-plugins` swaps the active
    /// `CommandRegistry`. The TUI overwrites
    /// `state.session.available_commands` with this list so the `/`
    /// autocomplete popup and command palette stay in sync. TS parity:
    /// the TS popup re-queries `getCommands()` inline; coco-rs pushes
    /// the snapshot because the registry lives in the CLI process.
    AvailableCommandsRefreshed {
        commands: Vec<crate::SlashCommandInfo>,
    },
    /// Queued command was removed from the engine queue and is ready to
    /// restore into the composer for editing.
    QueuedCommandEditReady {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<QueuedCommandEditImage>,
    },
    /// Editable queued commands were removed from the engine queue and
    /// combined with the current composer draft.
    QueuedCommandsEditReady {
        ids: Vec<String>,
        prompt: String,
        cursor: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<QueuedCommandEditImage>,
    },
    /// The requested queued command could not be restored.
    QueuedCommandEditUnavailable { id: String, reason: String },
    /// Open the resume picker inside the running TUI session.
    ///
    /// TS parity: `/resume` with no args opens the saved-chat picker;
    /// `/resume <id-or-name>` bypasses this and resumes directly.
    OpenSessionBrowser {
        sessions: Vec<crate::SdkSessionSummary>,
    },
    /// Rewind picker per-row metadata, emitted once on picker mount.
    ///
    /// One entry per real (non-synthetic) picker row, derived from
    /// the file-history snapshot pair `(this_user_message,
    /// next_user_message_or_now)`. `metadata == None` means
    /// `fileHistoryCanRestore(...)` was false (no snapshot) — the
    /// picker renders "⚠ No code restore" for that row.
    /// `metadata == Some { file_paths: [], .. }` means the snapshot
    /// exists but nothing changed — picker renders "No code changes".
    ///
    /// Bundled here so the bounded TUI command channel does not drop individual
    /// row events on transcripts with many user messages.
    RewindRowMetadataReady { rows: Vec<RewindRowMetadata> },
    /// Rewind restore-preview diff stats for the selected message,
    /// emitted after the user confirms a checkpoint pick. `stats ==
    /// None` means `fileHistoryCanRestore(...)` was false — the
    /// option screen suppresses code-restore choices and shows the
    /// conversation-only path.
    RewindRestorePreviewReady {
        message_id: String,
        stats: Option<RewindDiffStatsPayload>,
    },

    // === Compaction / speculation toasts (4) ===
    /// Compaction circuit breaker opened.
    CompactionCircuitBreakerOpen { failures: i32 },
    /// Micro-compaction applied notification.
    MicroCompactionApplied { removed: i32 },
    /// Session memory compaction applied notification.
    SessionMemoryCompactApplied { summary_tokens: i32 },
    /// Speculative execution rolled back.
    SpeculativeRolledBack { reason: String },

    // === Memory extraction toasts (3) ===
    /// Memory extraction started.
    SessionMemoryExtractionStarted,
    /// Memory extraction completed.
    SessionMemoryExtractionCompleted { extracted: i32 },
    /// Memory extraction failed.
    SessionMemoryExtractionFailed { error: String },

    // === Cron toasts (2) ===
    /// Cron job disabled by circuit breaker.
    CronJobDisabled { job_id: String, reason: String },
    /// Missed cron job fires.
    CronJobsMissed { count: i32 },

    // === Streaming tool display (3) ===
    /// Streaming tool input delta (typing effect).
    ///
    /// A tool call's arguments have started streaming (the model began
    /// emitting the input JSON). Carries the `call_id` and tool `name` so the
    /// TUI can create a pre-queue "streaming" row in the activity strip — the
    /// `call_id` matches the later [`AgentStreamEvent::ToolUseQueued`], which
    /// upgrades the same row with the finalized input once parsing completes.
    ///
    /// Emitted by the engine from `StreamEvent::ToolCallStart`. Like
    /// [`Self::ToolCallDelta`] it is UI-only: the SDK sees the complete input
    /// at `ToolUseQueued` and does not need the streaming preamble.
    ToolCallStreamStart { call_id: String, name: String },
    /// Partial tool-call input JSON fragment, for the live "typing" preview in
    /// the activity strip. The TUI appends the delta to
    /// `ToolExecution.streaming_input` and re-derives the primary-argument
    /// preview (the command for `Bash`, the path for `Read`, …) so the row
    /// shows arguments filling in before the call is queued.
    ///
    /// Emitted by the engine from `StreamEvent::ToolCallDelta` (a different,
    /// inference-internal type). Kept in `TuiOnlyEvent` rather than
    /// `AgentStreamEvent` per `event-system-design.md` §3.3: the partial JSON
    /// serves a purely UI display purpose — `ToolUseQueued` already contains
    /// the complete, re-assembled input, so SDK consumers need not handle the
    /// fragments.
    ToolCallDelta { call_id: String, delta: String },
    /// Tool progress update (progress bar).
    ToolProgress {
        tool_use_id: String,
        data: serde_json::Value,
    },
    /// Whether a streaming Enter can submit-interrupt the current turn.
    ToolInterruptibilityChanged { interruptible: bool },
    /// Tool execution aborted notification.
    ToolExecutionAborted {
        tool_use_id: String,
        reason: ToolAbortReasonPayload,
    },

    // === coco-rs extensions (not in the design's 20) ===
    /// Rewind completed — TUI truncates messages and restores input state.
    /// coco-rs extension: UI-only because it carries TUI-specific identifiers
    /// for message truncation and input repopulation. Out-of-band from the
    /// design's `rewind/completed` ServerNotification which carries protocol
    /// metadata only.
    RewindCompleted {
        /// UUID of the target user message. Empty = code-only rewind.
        target_message_id: String,
        /// Number of files restored (0 if conversation-only).
        files_changed: i32,
    },
    /// Local slash-command produced a `CommandResult::Text` (or
    /// `Compact { display_text }`). Surfaced as a system-role chat message
    /// so it reads inline with the transcript. `text` is the pre-rendered
    /// body — never translated, since it carries the handler's actual
    /// output (often command-specific status / git output / prompt
    /// preview).
    SlashCommandResult {
        name: String,
        /// Raw argument string the user typed (e.g. `sonnet` for
        /// `/model sonnet`). Threaded into the model-visible echo
        /// (`❯ /name args`) so the model sees the full invocation, matching
        /// TS `formatCommandInput(command, args)`. Empty for argless commands.
        args: String,
        text: String,
    },
    /// Open the full-color `/context` usage surface. Carries the analyzed
    /// report (the TUI cannot reach the engine to compute it). Replaces the
    /// former markdown-in-transcript output and the removed `Ctrl+W` overlay.
    OpenContextUsage {
        result: crate::server_request::ContextUsageResult,
    },
    /// Dispatcher-side breadcrumb for slash commands the runtime couldn't
    /// fully execute (missing handler, handler error, empty Prompt body,
    /// dialog wiring pending). The TUI translates `kind` via the i18n
    /// catalog before rendering.
    SlashCommandStatus {
        name: String,
        /// Argument string the user typed after the command, used to render the
        /// `❯ /name args` echo alongside the status. Empty when none was given.
        args: String,
        kind: SlashCommandStatusKind,
    },
    /// Tell the TUI to open the rewind picker overlay.
    ///
    /// Emitted by the slash-command dispatcher when `/rewind` resolves
    /// through the generic `DialogSpec` mapping. The TUI consumes the
    /// current transcript cells to build the bare picker; preselected
    /// rewind remains an internal UI command path.
    OpenRewindPicker,
    /// Tell the TUI to open the `/memory` file-picker overlay. The slash
    /// dispatcher pre-builds entries via
    /// `coco_commands::handlers::memory_dialog::MemoryDialogHandler::entries`
    /// and carries them through here so the TUI doesn't recompute paths.
    /// On selection the TUI sends an `OpenMemoryFile` command to the CLI
    /// bridge; the bridge creates the file (`mode wx` semantics), opens
    /// `$VISUAL || $EDITOR`, and reports the result back as a transcript
    /// visible event. On cancel the TUI emits "Cancelled memory editing".
    /// TS parity: `commands/memory/memory.tsx`.
    OpenMemoryDialog { entries: Vec<MemoryDialogEntry> },
    /// `/copy [N]` slash command — the TUI walks its transcript, picks
    /// the Nth-latest assistant message, and either directly copies it
    /// (when there are no code blocks or `copy_full_response` is on) or
    /// opens the [`ModalState::CopyPicker`] surface. The CLI runner
    /// emits this through `dispatch_slash_command`'s `/copy` intercept;
    /// only the TUI has the transcript in a render-ready shape.
    /// TS parity: `commands/copy/copy.tsx`.
    CopyCommandRequested {
        /// Raw arg string after `/copy`; empty when the user typed
        /// `/copy` with no argument. The TUI parses it as `usize` and
        /// surfaces a usage toast on invalid input.
        args: String,
    },
    /// `/memory` editor launch completed successfully.
    MemoryFileOpened {
        /// Path of the file passed to the editor.
        path: String,
    },
    /// `/memory` editor launch failed before a process was started.
    MemoryFileOpenFailed {
        /// Path of the file the user selected.
        path: String,
        /// User-visible failure summary.
        error: String,
    },
    /// Plan editor launch completed successfully.
    PlanFileOpened {
        /// Path of the session plan file passed to the editor.
        path: String,
    },
    /// Plan editor launch failed before a process was started.
    PlanFileOpenFailed {
        /// Path of the session plan file.
        path: String,
        /// User-visible failure summary.
        error: String,
    },
    /// Request the foreground TUI to leave raw mode / alt-screen before
    /// the CLI runner starts an interactive editor process.
    ExternalEditorPrepare {
        /// Opaque request id echoed back by the TUI once terminal modes
        /// are ready for the external editor.
        request_id: String,
    },
    /// External prompt editor completed.
    PromptEditorCompleted {
        /// Edited prompt content read back from the temp file.
        content: String,
        /// Whether the edited content differs from the initial content.
        modified: bool,
    },
    /// External prompt editor failed before content could be read back.
    PromptEditorFailed {
        /// User-visible failure summary.
        error: String,
    },
    /// Result of a prompt-mode `!`-prefixed bash submission. The TUI
    /// folds this into a `MessageContent::BashOutput` chat message
    /// keyed by the same `user_message_id` as the matching
    /// `BashInput`. Emitted by the CLI bridge after running the
    /// command via `coco_shell::ShellExecutor`. TS parity:
    /// `LocalShellTask` completion.
    BashCommandCompleted {
        /// Shared id of the bash input/output pair so rewind groups them.
        user_message_id: String,
        /// Merged stdout + stderr, already truncated to a reasonable
        /// display size by the bridge.
        output: String,
        /// Process exit code; non-zero shades the output red.
        exit_code: i32,
    },
    /// Tell the TUI to open the provider-grouped model picker. Emitted
    /// when the slash dispatcher resolves `/model` with no args (typed
    /// `/model` from input bar). The TUI consumes the current
    /// `state.session.model` to mark the "current" entry.
    OpenModelPicker,
    /// Tell the TUI to open the tabbed settings panel. Emitted when the slash
    /// dispatcher resolves `/config` with no args (TS `/config` local-jsx
    /// panel). Reuses the same overlay as the `Ctrl+,` keybind.
    OpenSettings,
    /// Tell the TUI to open the standalone theme picker. Emitted when the
    /// slash dispatcher resolves `/theme` with no args. The TUI builds the
    /// choice list from its live `ThemeRuntimeState`.
    OpenThemePicker,
    /// Tell the TUI to open the `/skills` read-only overlay. The slash
    /// dispatcher pre-builds the entry list + per-group subtitles so
    /// the TUI doesn't recompute paths or token estimates.
    ///
    /// TS parity: `commands/skills/skills.tsx` → `<SkillsMenu>`. Dialog
    /// is read-only — Esc to close; selection has no side effects.
    OpenSkillsDialog { payload: SkillsDialogPayload },
    /// `/plugin` overlay — opens the tabbed installed / marketplace / errors
    /// manager. The payload is a CLI-built snapshot because the TUI cannot
    /// reach plugin loader state directly.
    OpenPluginDialog { payload: PluginDialogPayload },
    /// `/agents` overlay — opens the 2-tab `<AgentsDialog>` with the
    /// Library entries pre-grouped. The Running tab is sourced
    /// directly from `SessionState.subagents` on the TUI side.
    OpenAgentsDialog { payload: AgentsDialogPayload },
    /// `/permissions` overlay — opens the tabbed rule editor
    /// (Allow / Ask / Deny / Workspace). The payload is a CLI-built
    /// snapshot of every file-backed rule + additional directory, since
    /// the TUI cannot reach the settings stores directly. Re-emitted
    /// after each edit so the open overlay refreshes in place. TS parity:
    /// `commands/permissions/permissions.tsx` → `<PermissionRuleList>`.
    OpenPermissionsEditor { payload: PermissionsEditorPayload },
    /// Open the interactive `/add-dir` overlay (no-argument form). The TUI
    /// shows a directory-path text input; confirm re-dispatches
    /// `/add-dir <path>` to reuse the validated session-add path. TS parity:
    /// `commands/add-dir/add-dir.tsx` → `<AddWorkspaceDirectory>`.
    OpenAddDirectory,
    /// Open the interactive `/export` format picker (no-argument form). The
    /// TUI shows the Markdown/JSON/Text chooser; confirm re-dispatches
    /// `/export <format>`, which the CLI runner renders from the live
    /// `MessageHistory` and writes to disk. (coco-specific: TS's
    /// `<ExportDialog>` instead offers clipboard-vs-file for a single plain-text
    /// format; coco picks among Markdown/JSON/Text and writes to a file.)
    OpenExport,
    /// Notify the TUI that a `/skills` dialog Enter has finished
    /// persisting (or failed). TUI renders the localized
    /// `Updated N / No changes / Failed: …` toast — keeping all
    /// user-visible text generation on the UI side.
    SkillOverridesSaved { result: SkillOverridesSaveResult },
}

/// Image payload paired with a queued command edit restore.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedCommandEditImage {
    pub media_type: String,
    pub data_base64: String,
}

/// Outcome of a `/skills` dialog save dispatch. CLI bridge populates
/// this after `SettingsWriter::write_local`.
///
/// Carries **no display data** — TUI owns toast text generation and
/// stashes the pre-write `total_edits` count on its own state
/// before dispatching. CLI only reports success vs typed failure.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SkillOverridesSaveResult {
    /// Write succeeded.
    Ok,
    /// Write failed at some step. `kind` lets the TUI pick toast
    /// severity / wording; `message` is the underlying error's
    /// display text for inclusion in the toast body.
    Err {
        kind: SkillOverridesSaveErrorKind,
        message: String,
    },
}

/// Categorical save-failure source. Mirrors the
/// [`coco_config::SettingsWriteError`] variants + a runtime tier
/// for "settings hot-reload was disabled at session start so we
/// can't republish".
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOverridesSaveErrorKind {
    /// Filesystem I/O failed.
    Io,
    /// Settings JSON parse / serialize failed.
    Parse,
    /// `RuntimeConfig` rebuild failed after the write.
    Rebuild,
    /// Hot-reload publisher is absent (one-shot build path).
    NoPublisher,
}

/// One row in the `/memory` file-picker overlay. Built by the slash
/// dispatcher and shipped to the TUI via [`TuiOnlyEvent::OpenMemoryDialog`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDialogEntry {
    /// Filesystem path of the memory file (also the editor target).
    pub path: String,
    /// Display label shown in the picker row.
    pub label: String,
    /// Scope tag (drives ordering and color hint).
    pub scope: MemoryDialogScope,
    /// Row semantics for rendering and selection behavior.
    #[serde(default = "default_memory_dialog_row_kind")]
    pub row_kind: MemoryDialogRowKind,
}

fn default_memory_dialog_row_kind() -> MemoryDialogRowKind {
    MemoryDialogRowKind::File {
        exists: false,
        read_only: false,
    }
}

/// Semantic row kind for the `/memory` picker.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryDialogRowKind {
    /// Editable memory file.
    File {
        /// Whether the file already exists at event construction time.
        #[serde(default)]
        exists: bool,
        /// Whether the row should be treated as read-only by future UIs.
        #[serde(default)]
        read_only: bool,
    },
    /// Folder grouping row for future memory directory surfaces.
    Folder {
        /// Whether the folder is currently enabled.
        #[serde(default)]
        enabled: bool,
    },
    /// Toggle row for future auto-memory settings.
    Toggle {
        /// Current toggle state.
        #[serde(default)]
        enabled: bool,
    },
}

/// Scope tag for a memory file picker entry. Mirrors
/// `coco_commands::MemoryScope` — kept in `coco-types` so the TUI can
/// consume the event without depending on `coco-commands`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDialogScope {
    /// Enterprise / managed (typically read-only for the user).
    Managed,
    /// User-global (`~/.coco/CLAUDE.md` or `~/CLAUDE.md`).
    User,
    /// Project (`./CLAUDE.md`).
    Project,
    /// Project-local (`./CLAUDE.local.md`, gitignored).
    ProjectLocal,
    /// `<dir>/.coco/CLAUDE.md` — project-config-dir convention.
    ProjectConfig,
    /// Subdirectory CLAUDE.md (auto-loaded under cwd).
    Subdir,
    /// File loaded transitively via `@-import` from a parent memory file.
    Imported,
    /// Auto-memory directory entry (`<memdir>/`).
    AutoMemFolder,
    /// Team memory directory entry (`<memdir>/team/`).
    TeamMemFolder,
    /// Per-agent memory directory entry.
    AgentMemFolder,
}

/// Per-skill override state stored under `skill_overrides` in any
/// settings tier. Drives the `/skills` 4-state editor ladder.
///
/// Wire format is kebab-case (`"on"`, `"name-only"`,
/// `"user-invocable-only"`, `"off"`) — JSON settings files round-trip
/// without translation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillOverrideState {
    /// Default — full description in model listing, both user `/` and
    /// model Skill-tool invocation allowed.
    On,
    /// Name-only listing (model sees `- name` without description);
    /// **model can still invoke**. Saves description tokens.
    NameOnly,
    /// Hidden from model listing; Skill tool rejects model invocation
    /// **unless** the user typed `/<name>` in the current turn. Slash
    /// dispatcher still works.
    UserInvocableOnly,
    /// Fully disabled — hidden from listing AND `/` autocomplete;
    /// Skill tool rejects every invocation attempt.
    Off,
}

impl SkillOverrideState {
    /// Cycle to the next state in the TS 4-state ladder
    /// (`on → name-only → user-invocable-only → off → on`). Used by
    /// the `/skills` dialog Space key.
    pub const fn next(self) -> Self {
        match self {
            Self::On => Self::NameOnly,
            Self::NameOnly => Self::UserInvocableOnly,
            Self::UserInvocableOnly => Self::Off,
            Self::Off => Self::On,
        }
    }
}

/// Which precedence layer originated a non-overridable lock on a
/// skill's `skill_overrides` state. Mirrors the four `lock.source`
/// values returned by TS `oT5` (`cli_inner_pretty.js:476885-476893`).
///
/// In precedence order (highest first): [`Self::Policy`] →
/// [`Self::Flag`] → [`Self::Author`] → [`Self::Plugin`]. A lock means
/// the `/skills` dialog renders `🔒 <label>` for the row and refuses
/// to cycle it (Space is a no-op).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLockSource {
    /// `policySettings.skill_overrides[name]` — enterprise-managed.
    Policy,
    /// `flagSettings.skill_overrides[name]` — `--settings <path>`
    /// invocation override.
    Flag,
    /// Skill frontmatter `disable-model-invocation: true` — author
    /// forced to `user-invocable-only`.
    Author,
    /// `skill.source == Plugin` — plugin-contributed skills are
    /// forced to `on` (manage via `/plugin` instead).
    Plugin,
}

/// A non-overridable lock on a skill row in the `/skills` dialog.
/// Carries both the originating tier ([`Self::source`]) and the
/// forced 4-state value ([`Self::forced_value`]) so downstream
/// renderers don't need to re-derive the value from per-tier maps.
///
/// TS mirror: `oT5` returns `{ value, source }` —
/// `cli_inner_pretty.js:476885-476893`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLock {
    pub source: SkillLockSource,
    pub forced_value: SkillOverrideState,
}

/// Payload for [`TuiOnlyEvent::OpenPluginDialog`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDialogPayload {
    pub installed: Vec<PluginDialogInstalledRow>,
    pub marketplaces: Vec<PluginDialogMarketplaceRow>,
    pub errors: Vec<PluginDialogErrorRow>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDialogInstalledRow {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: String,
    pub path: String,
    pub enabled: bool,
    pub blocked_by_policy: bool,
    #[serde(default)]
    pub options: Vec<PluginDialogOptionRow>,
    #[serde(default)]
    pub mcp_servers: Vec<PluginDialogMcpServerRow>,
    #[serde(default)]
    pub actions: Vec<PluginDialogAction>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDialogMarketplaceRow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub official: bool,
    pub plugin_count: i64,
    #[serde(default)]
    pub actions: Vec<PluginDialogAction>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDialogErrorRow {
    pub plugin_id: String,
    pub message: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDialogOptionRow {
    pub key: String,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDialogMcpServerRow {
    pub name: String,
    pub display_name: String,
    pub enabled: bool,
    pub needs_config: bool,
    #[serde(default)]
    pub tools: Vec<PluginDialogMcpToolRow>,
    #[serde(default)]
    pub actions: Vec<PluginDialogAction>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDialogMcpToolRow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDialogAction {
    pub label: String,
    /// Slash command args after `/plugin`.
    pub plugin_args: String,
}

/// Payload for [`TuiOnlyEvent::OpenSkillsDialog`]. Built once by the
/// `/skills` slash handler so the TUI doesn't recompute paths, token
/// estimates, or grouping.
///
/// TS parity: 2.1.142 `uJ4` (`cli_inner_pretty.js:476909`) — a flat
/// editable list with 4-state override cycling, source labels
/// inline, and lock annotations for policy/flag/author/plugin-locked
/// rows. The 2.1.88 grouped read-only `SkillsMenu` has been retired.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsDialogPayload {
    /// All visible skills, in arbitrary order. The renderer
    /// applies its own sort (source-string lex + name; or token-
    /// descending when the user pressed `t`). Total count is
    /// `entries.len()` (drives the subtitle `{N} skills`).
    pub entries: Vec<SkillsDialogEntry>,
    /// Bytes per token for the current main-role model. The TUI
    /// divides [`SkillsDialogEntry::frontmatter_bytes`] by this to
    /// render the `~N tok` column. Set to 4 when the host cannot
    /// resolve a more accurate value — `bytes/token ≈ 4` is the
    /// English-text rule-of-thumb fallback.
    pub bytes_per_token: i64,
}

/// One row in the `/skills` dialog. Every field is required so the dialog
/// never has to fabricate defaults for `baseline` / `lock` / etc.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsDialogEntry {
    /// Canonical skill name — what `/<name>` invokes.
    pub name: String,
    /// Source group this entry belongs to. Drives the source label
    /// rendered inline + the implicit alphabetical group when the
    /// default sort is active.
    pub source: SkillsDialogSource,
    /// One-line description from the skill frontmatter. The 2.1.142
    /// filter (`/` search) matches name + description + source
    /// label, so the dialog needs the description on the wire even
    /// though it isn't shown on every row.
    pub description: String,
    /// Plugin name shown inline when `source == Plugin`. None
    /// otherwise. TS: `skill.pluginInfo?.pluginManifest.name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    /// Byte length of the frontmatter the token column derives from.
    /// The dialog computes `frontmatter_bytes / bytes_per_token` per
    /// row. Source: `coco_skills::estimate_skill_frontmatter_bytes`.
    pub frontmatter_bytes: i64,
    /// What is stored in `<cwd>/.coco/settings.local.json`'s
    /// `skill_overrides[name]` _right now_. `None` ⇒ key absent.
    /// Drives the dialog's diff-against-baseline save algorithm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_local: Option<SkillOverrideState>,
    /// Project-or-user settings resolution, **without** local /
    /// policy / flag layers. TS `aT5`. The dialog falls back here
    /// when the user reverts a local override (the save path
    /// writes `Value::Null` so the local key is deleted and the
    /// baseline resurfaces).
    pub baseline: SkillOverrideState,
    /// Optional lock — present when this row's state is forced by
    /// a higher-precedence layer. Set by
    /// `resolve_skill_override_lock` (`oT5` mirror). When set, the
    /// dialog renders `🔒 <label>` and no-ops on Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<SkillLock>,
}

/// Source group for a skill dialog entry. Collapsed from
/// `SettingSource | 'plugin' | 'mcp'` to a closed enum so the wire shape is statically typed.
///
/// The dialog normalises `bundled`/`builtin` → display label `"built-in"`; the
/// dialog filter matches against that lowercased label.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillsDialogSource {
    /// Compiled-in bundled skill catalog. TS `bundled` / `builtin`
    /// collapsed to a single label (display: `built-in`).
    BuiltIn,
    /// `<cwd>/.coco/skills/` — project skills.
    Project,
    /// `~/.coco/skills/` — TS `userSettings`.
    User,
    /// Managed enterprise dir — TS `policySettings`.
    Policy,
    /// Skills contributed by a loaded plugin.
    Plugin,
    /// Skills published by a connected MCP server.
    Mcp,
}

/// Payload for [`TuiOnlyEvent::OpenAgentsDialog`]. Built by the
/// `/agents` slash handler with everything the 2-tab dialog needs
/// (Running tab reads `SessionState.subagents` directly, so the
/// payload only carries Library data).
///
/// TS parity: bundled-only `cli_unpack_pretty/decls/functions/bW4.js`
/// (Library tab). The open-source `<AgentsMenu>` exposes the same
/// per-row data but routes through a different state machine.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsDialogPayload {
    /// All visible agents, source-grouped at dialog-build time so the
    /// TUI doesn't recompute precedence. Order: User → Project →
    /// Local → Managed → Plugin → Flag → Built-in. Empty groups are
    /// omitted.
    pub entries: Vec<AgentsDialogEntry>,
}

/// One row in the `/agents` Library tab.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsDialogEntry {
    /// Canonical `agent_type` identifier.
    pub name: String,
    /// `whenToUse` description (single line). Empty when frontmatter
    /// omitted it — the renderer falls back to a placeholder.
    pub description: String,
    /// Which source loaded this entry.
    pub source: crate::AgentSource,
    /// Optional badge color from frontmatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<crate::AgentColorName>,
    /// `true` when this `agent_type` is shadowed by a higher-priority
    /// source (still loadable, but the active resolution picks the
    /// override). TS: `is_overridden` flag in `loadAgentsDir.ts`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_overridden: bool,
    /// Absolute markdown source path. `None` for built-in / plugin /
    /// in-memory entries that aren't editable via `$EDITOR`. Drives
    /// the Library tab's `Enter` (edit) and `d` (delete) actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<std::path::PathBuf>,
}

/// Payload for [`TuiOnlyEvent::OpenPermissionsEditor`]. Built by the
/// `/permissions` slash handler (CLI side) with everything the tabbed
/// rule editor needs: every file-backed rule keyed by behavior + source,
/// every additional working directory, and the current working directory.
///
/// The TUI cannot read the settings stores itself, so the CLI snapshots
/// them here and re-emits this payload after each persisted edit (the
/// open overlay refreshes in place, mirroring `OpenAgentsDialog`).
///
/// TS parity: data backing `<PermissionRuleList>` —
/// `getPermissionRules()` + `getAdditionalWorkingDirectories()`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsEditorPayload {
    /// Every file-backed rule (allow / ask / deny) across the user,
    /// project, local, and policy settings layers. The TUI partitions
    /// them by behavior into the Allow / Ask / Deny tabs.
    pub rules: Vec<PermissionsEditorRule>,
    /// Additional working directories configured in settings, each
    /// tagged with the source layer that contributed it.
    pub directories: Vec<PermissionsEditorDir>,
    /// Current working directory — rendered read-only at the top of the
    /// Workspace tab (TS "Original working directory").
    pub cwd: String,
    /// `true` when managed policy sets `allowManagedPermissionRulesOnly`.
    /// The editor renders every rule read-only and blocks add / delete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed_only: bool,
}

/// One rule row in the `/permissions` editor. Carries the structured
/// `(tool_pattern, rule_content)` so the TUI can both render the rule
/// string and reconstruct a `PermissionRuleValue` for removal — display
/// shaping (and i18n) stays TUI-side.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsEditorRule {
    pub behavior: crate::PermissionBehavior,
    pub source: crate::PermissionRuleSource,
    /// Tool the rule targets — e.g. `"Bash"`, `"Read"`, `"mcp__slack__*"`.
    pub tool_pattern: String,
    /// Optional in-tool content pattern — e.g. `"git *"` for `Bash(git *)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

/// One additional-working-directory row in the Workspace tab.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsEditorDir {
    pub path: String,
    /// Source layer that contributed the directory (drives the inline
    /// source label and the read-only gate for policy entries).
    pub source: crate::PermissionRuleSource,
}

/// Categorization of a `SlashCommandStatus` payload. Each variant maps to
/// a `slash.status.*` key in the TUI locale catalog.
///
/// Wire format intentionally tagged so SDK clients can render their own
/// localized strings instead of consuming the TUI's English fallback.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlashCommandStatusKind {
    /// Registered command has no `handler` — typically a plugin
    /// contribution stub that wasn't bridged.
    NoHandler,
    /// Handler returned `Err`. `error` is the formatted error message.
    Failed { error: String },
    /// Handler returned `CommandResult::Prompt` with no text parts.
    EmptyPrompt,
    /// Handler returned `CommandResult::OpenDialog`, but coco-rs has not
    /// yet bound this dialog kind to a TUI overlay. `dialog_kind` is a
    /// human-readable label like "memory file selector".
    DialogPending { dialog_kind: String },
    /// `/permissions allow` invoked with no tool name. Dispatcher-side
    /// usage hint — the TUI translates via `slash.permissions.usage_allow`.
    PermissionsUsageAllow,
    /// `/permissions deny` invoked with no tool name. Dispatcher-side
    /// usage hint — the TUI translates via `slash.permissions.usage_deny`.
    PermissionsUsageDeny,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "event.test.rs"]
mod tests;
