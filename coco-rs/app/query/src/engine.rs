//! The agent loop — heart of the system.
//!
//! State transitions tracked via ContinueReason to enable tests to verify
//! recovery paths without inspecting message contents.

use crate::budget::BudgetDecision;
use crate::command_queue::CommandQueue;
use crate::emit::emit_protocol;
use crate::session_state::SessionStateTracker;
use coco_context::FileHistoryState;
use coco_hooks::HookRegistry;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_messages::CostTracker;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::create_plan_implementation_message;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::TurnAbortSignal;
use coco_types::TokenUsage;
use coco_types::ToolAppState;

use crate::helpers::convert_to_assistant_content;
use crate::helpers::extract_last_assistant_text;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::ReasoningPart;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

pub use crate::config::ContinueReason;
pub use crate::config::QueryEngineConfig;
pub use crate::config::QueryResult;
pub use crate::config::SessionBootstrap;
use crate::engine_result::make_query_result;

/// Last-compact tracker for `RecompactionInfo` population. Set by
/// `try_full_compact` after a successful compact and read by the next
/// compaction to derive `is_recompaction` / `turns_since_previous`.
///
/// Read directly, no subtraction needed:
/// - `run_id` — UUID generated per compact.
/// - `turn_counter` — resets to 0 on each compact, bumped +1 per
///   subsequent turn at `engine_finalize_turn.rs`.
#[derive(Debug, Clone)]
pub(crate) struct LastCompactState {
    /// Turns elapsed since the previous compact. `0` immediately after
    /// the compact lands; incremented on each `finalize_turn_post_tools`.
    /// Read directly as `RecompactionInfo.turns_since_previous`.
    pub(crate) turn_counter: i64,
    /// UUID-shaped id of the previous compaction (boundary marker uuid).
    /// Surfaced through `coco_query::compact_track` tracing log; Rust's
    /// substitute for `tengu_post_autocompact_turn` analytics.
    pub(crate) run_id: String,
}

/// The query engine — orchestrates multi-turn agent conversations.
///
/// Fields are `pub(crate)` so the impl block can be split across sibling
/// modules (`engine_builder`, `engine_session`, `engine_compaction`, …)
/// without leaking internal state to the public API. External callers see
/// only the methods exposed by these impls, never the fields directly.
pub struct QueryEngine {
    pub(crate) config: QueryEngineConfig,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) cancel: CancellationToken,
    pub(crate) turn_abort: TurnAbortSignal,
    pub(crate) hooks: Option<Arc<HookRegistry>>,
    /// Captures `is_async` hook output so the reminder pipeline can
    /// deliver it on later turns. Wired by `engine_builder` from the
    /// shared `SessionRuntime`-owned `AsyncHookRegistry`. `None` means
    /// async hooks fired through this engine are dropped — the
    /// session-runtime path stays the canonical wiring.
    pub(crate) async_hook_registry: Option<Arc<coco_hooks::async_registry::AsyncHookRegistry>>,
    /// LLM-driven hook handler used by `Prompt` / `Agent` hook
    /// handlers. Wired by `engine_builder` from the
    /// `SessionRuntime`-owned `Arc<dyn HookLlmHandle>`. `None` means
    /// LLM-driven handlers fall back to passthrough text — orchestration
    /// already handles that path with a `tracing::warn!`.
    pub(crate) hook_llm_handle: Option<Arc<dyn coco_hooks::HookLlmHandle>>,
    /// Shared model runtime registry for all LLM calls.
    pub(crate) model_runtimes: Arc<ModelRuntimeRegistry>,
    /// Runtime source used for this engine's agent-loop calls.
    pub(crate) model_runtime_source: ModelRuntimeSource,
    /// Pending tool-use-summary `JoinHandle` produced by
    /// [`finalize_turn_post_tools`](Self::finalize_turn_post_tools).
    /// Awaited at the top of the *next* `run_session_loop` iteration
    /// so the summary surfaces to SDK consumers as a
    /// `ServerNotification::ToolUseSummary` just before the next API
    /// call.
    ///
    /// `Arc<Mutex>` (not session-loop local) because the spawn site
    /// (`finalize_turn_post_tools`) and await site (`run_session_loop`
    /// iteration top) sit on different `&self` boundaries; the lock
    /// is uncontended in practice — at most one entry transition per
    /// turn.
    pub(crate) pending_tool_use_summary: Arc<
        tokio::sync::Mutex<
            Option<tokio::task::JoinHandle<Option<coco_types::ToolUseSummaryParams>>>,
        >,
    >,
    /// Mid-turn command queue for steering. Carries both human-typed
    /// prompts (via `QueueOrigin::Human`) and teammate / task-notification
    /// messages (via `QueueOrigin::Coordinator` / `QueueOrigin::TaskNotification`).
    pub(crate) command_queue: CommandQueue,
    /// Session-level file read state for @mention dedup and changed-file detection.
    pub(crate) file_read_state: Option<Arc<RwLock<coco_context::FileReadState>>>,
    /// File history for checkpoint/rewind.
    pub(crate) file_history: Option<Arc<RwLock<FileHistoryState>>>,
    /// Config home directory for file history backup storage.
    pub(crate) config_home: Option<std::path::PathBuf>,
    /// One-shot SessionStarted payload; emitted at the first turn entry.
    pub(crate) session_bootstrap: Option<SessionBootstrap>,
    /// Optional permission bridge for routing `PermissionDecision::Ask`
    /// outcomes to an external authority (swarm leader or SDK client).
    /// `None` uses the engine's fallback auto-allow behavior.
    pub(crate) permission_bridge: Option<coco_tool_runtime::ToolPermissionBridgeRef>,
    /// Auto-mode state + rules for the 2-stage LLM classifier. When active,
    /// tool calls that return `PermissionDecision::Ask` are first run through
    /// `can_use_tool_in_auto_mode` — Allow/Deny short-circuits the permission
    /// bridge; None falls through to interactive approval.
    pub(crate) auto_mode_state: Option<Arc<coco_permissions::AutoModeState>>,
    pub(crate) denial_tracker: Option<Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
    pub(crate) auto_mode_rules: coco_permissions::AutoModeRules,
    /// Shared cross-turn app state (typed) — carries flags like
    /// `needs_plan_mode_exit_attachment` set by `ExitPlanModeTool`.
    /// Attached via [`Self::with_app_state`]; absent on engines that
    /// don't need this signalling.
    pub(crate) app_state: Option<Arc<RwLock<ToolAppState>>>,
    /// Mailbox handle for swarm teammate messaging. `None` resolves to
    /// `NoOpMailboxHandle` in [`ToolContextFactory::build`]; swarm spawn paths
    /// install a real handle via [`Self::with_mailbox`].
    pub(crate) mailbox: Option<coco_tool_runtime::MailboxHandleRef>,
    /// Per-recipient pending-message store. `None` resolves to
    /// `NoOpPendingMessageStore` in [`ToolContextFactory::build`].
    /// Production wires an `InMemoryPendingMessageStore` here AND on
    /// the SwarmAdapter — single source of truth across SendMessage
    /// push and `agent_pending_messages` reminder drain.
    pub(crate) pending_messages: Option<coco_tool_runtime::PendingMessageStoreRef>,
    /// MCP handle used at prompt-rendering time to read the connected
    /// server set so `AgentTool::prompt` can pre-filter agents whose
    /// `required_mcp_servers` aren't yet ready. `None` ⇒ filter is
    /// skipped (the renderer treats absent MCP as "no filter
    /// applies"). Wired by `session_runtime` via
    /// [`Self::with_mcp_handle`].
    pub(crate) mcp_handle: Option<coco_tool_runtime::McpHandleRef>,
    /// Scheduling backend for Cron*/RemoteTrigger tools.
    pub(crate) schedule_store: Option<coco_tool_runtime::ScheduleStoreRef>,
    /// LSP handle for code-intelligence operations exposed to tools
    /// (`LSPTool`). `None` ⇒ `ToolContextFactory` substitutes
    /// `NoOpLspHandle`, which reports `is_connected() = false` so
    /// `LspTool::is_enabled` filters the tool out of the model's tool
    /// list. Wired by `session_runtime` via [`Self::with_lsp_handle`].
    pub(crate) lsp_handle: Option<coco_tool_runtime::LspHandleRef>,
    /// Agent-runtime handle for `AgentTool` (subagent spawn / team
    /// management / background signalling). `None` resolves to
    /// `NoOpAgentHandle` in [`ToolContextFactory::build`]; the CLI /
    /// SDK / TUI runners install a real handle via
    /// [`Self::with_agent_handle`] so `AgentTool` calls reach the
    /// swarm runtime. Sessions that skip installation intentionally
    /// restrict Agent tools to model-visible errors.
    pub(crate) agent_handle: Option<coco_tool_runtime::AgentHandleRef>,
    /// Tool-result budget replacement state. Threaded through every
    /// per-turn `apply_tool_result_budget` call so seen_ids freeze
    /// across turns (a result, once seen, is never re-evaluated for
    /// replacement). The `per_message_chars` field is overwritten per
    /// call from the live `compact.tool_result_budget` so the budget
    /// reflects hot-reloaded config.
    pub(crate) tool_result_replacement_state:
        coco_tool_runtime::tool_result_storage::ContentReplacementStateRef,
    /// Skill-runtime handle for `SkillTool`. Phase 7 routed skills
    /// off `AgentHandle::resolve_skill` onto this dedicated trait.
    /// `None` resolves to `NoOpSkillHandle` in the factory, which
    /// returns `SkillInvocationError::Unavailable` — the runner
    /// surfaces that as a clean model-visible error rather than
    /// panicking.
    pub(crate) skill_handle: Option<coco_tool_runtime::SkillHandleRef>,
    /// Skill-emitted Command-source permission rules scoped to **this
    /// engine instance**. Since `QueryEngine` is rebuilt fresh per user
    /// message (`SessionRuntime::build_engine` is called from every
    /// TUI / SDK / headless / fork driver), engine-scoped = user-msg-scoped.
    /// Scoped command-allow rules — engine-scoped = user-msg-scoped.
    /// Every turn's `ToolContextFactory::build` reads this Arc and
    /// merges contents into `ToolPermissionContext.allow_rules` under
    /// [`coco_types::PermissionRuleSource::Command`]:
    ///
    /// - **Within one user message** — every turn's
    ///   `ToolContextFactory::build` reads this Arc and merges its
    ///   contents into `ToolPermissionContext.allow_rules` under
    ///   [`coco_types::PermissionRuleSource::Command`], so a skill
    ///   invoked on turn N's auto-allow rules are honored on turn N+1.
    /// - **Across user messages** — the engine drops, the Arc count
    ///   goes to zero, the next user message's engine starts with a
    ///   fresh empty store.
    /// - **Subagent forks** — each forked engine builds its own
    ///   Arc; rules emitted inside a subagent skill cannot leak to
    ///   the parent.
    ///
    /// Shared by `Arc` with [`Self::permission_rule_handle`] (which
    /// writes into it) and with the per-batch `ToolContextFactory`
    /// (which reads it for the merge). See `engine_live_rules` module.
    pub(crate) live_command_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>>,
    /// Handle the executor installs on every batch so tool-emitted
    /// `permission_updates` flow into [`Self::live_command_rules`].
    /// Defaults to an [`crate::engine_live_rules::EngineLiveRulesHandle`]
    /// constructed from the same Arc; tests/standalone may override via
    /// [`Self::with_permission_rule_handle`] to install a `NoOp` for
    /// isolation.
    pub(crate) permission_rule_handle: coco_tool_runtime::PermissionRuleHandleRef,
    /// Snapshot of the agent-definition catalog (T7). Surfaced on
    /// every `ToolUseContext` so AgentTool can resolve
    /// `subagent_type → AgentDefinition` and thread the definition
    /// through `AgentSpawnRequest.definition`. `None` ⇒ AgentTool
    /// falls back to subagent_type→role mapping alone.
    pub(crate) agent_catalog: Option<std::sync::Arc<coco_subagent::AgentCatalogSnapshot>>,
    /// Post-turn cache-safe parameter slot (D8). Populated in
    /// `finalize_turn_post_tools` with the parameters that drove the
    /// turn's last request, so post-turn forks (`/btw`,
    /// `promptSuggestion`, `postTurnSummary`) can share the parent's
    /// prompt cache by sending byte-identical request prefixes. Read
    /// via [`Self::last_cache_safe_params`]. The slot lives and dies with
    /// the per-turn engine; `/clear` drops the observing handle at the
    /// runtime layer (see `SessionRuntime::clear_conversation`).
    ///
    /// `Arc<RwLock<...>>` so observers (TUI status, transcript recorder)
    /// can read the slot without contending with the engine's writer side.
    pub(crate) last_cache_safe_params:
        std::sync::Arc<tokio::sync::RwLock<Option<coco_types::CacheSafeParams>>>,
    /// Optional dispatcher for one-shot forked queries (D1/D2). When
    /// installed, post-turn forks (`/btw`, `promptSuggestion`,
    /// `postTurnSummary`) drive a *fresh* engine via this dispatcher
    /// rather than mutating the parent. Built and installed by the
    /// CLI bootstrap — TUI/SDK runners share the same instance.
    /// `None` ⇒ post-turn forks degrade to no-op (a placeholder is
    /// surfaced where appropriate; the parent loop continues).
    pub(crate) fork_dispatcher: Option<crate::forked_agent::ForkDispatcherRef>,
    /// Session-scoped abort slot for the in-flight prompt-suggestion
    /// fork. When the engine spawns a new suggestion fork, it cancels
    /// the previous in-flight one so rapid `/clear` cycles don't
    /// accumulate fork tasks burning tokens. `None` ⇒ no abort slot
    /// wired (test contexts).
    pub(crate) current_suggestion_abort:
        Option<std::sync::Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>>,
    /// Background task runtime — the [`TaskHandle`] consumed by
    /// `TaskGet` / `TaskOutput` / `TaskStop` / `TaskList` and by the
    /// AgentTool background dispatch (P2'+ TaskManager wiring).
    /// `None` resolves to `NoOpTaskHandle`; CLI bootstrap installs the
    /// production `TaskRuntime` shared with `SwarmAgentHandle` so a
    /// bg spawn registered by the latter is addressable through the
    /// former.
    pub(crate) task_handle: Option<coco_tool_runtime::BackgroundTaskHandleRef>,
    /// Persistent task-list store (V2, `TaskCreate`/`TaskUpdate`/etc.).
    /// `None` resolves to `NoOpTaskListHandle` — the V2 tools then
    /// return errors on write, matching TS's "no store configured"
    /// behavior. Install via [`Self::with_task_list`].
    pub(crate) task_list: Option<coco_tool_runtime::TaskListHandleRef>,
    /// Router for switching leader task tools onto a team task list.
    pub(crate) team_task_list_router: Option<coco_tool_runtime::TeamTaskListRouterRef>,
    /// Per-agent ephemeral todo store (V1, `TodoWrite`). Defaults to
    /// an in-memory instance when absent.
    pub(crate) todo_list: Option<coco_tool_runtime::TodoListHandleRef>,
    /// Bundle of per-subsystem reminder sources. Populated by CLI /
    /// SDK callers via [`Self::with_reminder_sources`]. Empty default
    /// ⇒ cross-crate reminders silently skip (matches TS behavior
    /// when the corresponding manager isn't initialized).
    pub(crate) reminder_sources: coco_system_reminder::ReminderSources,
    /// Channel for silent attachment events produced by owner crates
    /// (hooks, permissions, commands, core/tool-runtime, skills). Drained at the
    /// head of each outer-loop iteration so the `Message::Attachment`
    /// entries land in history before prompt build.
    ///
    /// Sender cloned to [`Self::attachment_emitter`] for plumbing into
    /// owner crates; receiver is drained by `drain_attachment_inbox`.
    pub(crate) attachment_tx: tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
    pub(crate) attachment_rx: Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<coco_messages::AttachmentMessage>>,
    >,
    /// Observers notified after each successful full compaction. Each
    /// crate that owns post-compact-invalidated state (file caches, skill
    /// state, memory caches) registers itself once at startup. Empty
    /// default ⇒ no observers fire. Implements post-compact cleanup
    /// as a pluggable registry.
    pub(crate) compaction_observers: Arc<coco_compact::CompactionObserverRegistry>,
    /// Wall-clock millis of the most recent assistant message. Drives
    /// `evaluate_time_based_trigger` so a long inactivity gap can clear
    /// stale tool results before the next API call.
    pub(crate) last_assistant_ms: Arc<std::sync::atomic::AtomicI64>,
    /// In-memory `lastSummarizedMessageId` carried by the session-memory
    /// path. Set after extraction completes; cleared after any compaction
    /// (the kept-tail UUIDs are no longer the same anchor).
    pub(crate) last_summarized_message_id: Arc<std::sync::Mutex<Option<uuid::Uuid>>>,
    /// Pre-extracted session memory summary text. Empty string disables
    /// the SM-first compact path.
    pub(crate) session_memory_text: Arc<tokio::sync::RwLock<String>>,
    /// Optional handle to the consolidated session-memory service.
    /// When present, `try_session_memory_compact` reads its cached
    /// body and calls `wait_for_extraction()` to avoid racing the
    /// in-flight forked-agent extractor. This is the same `Arc` as
    /// `memory_runtime.session_memory` when both are populated —
    /// wired by `SessionRuntime` for direct access without an
    /// `Option<MemoryRuntime>` hop.
    pub(crate) session_memory_service: Option<Arc<coco_memory::SessionMemoryService>>,
    /// Auto-memory runtime — extraction / dream / 9-section session
    /// memory / recall ranker. Distinct from
    /// `session_memory_service` above (which is the compact-side
    /// summary state). When `None`, the memory subsystem stays inert
    /// — typically only set when `Feature::AutoMemory` is enabled and
    /// the CLI bootstrap built the runtime. Turn-end hooks fan out
    /// through this one runtime.
    pub(crate) memory_runtime: Option<Arc<coco_memory::MemoryRuntime>>,
    /// Reactive (PTL) compaction circuit breaker. Tracks consecutive
    /// failures so we stop hammering the same recovery path after
    /// `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`.
    pub(crate) reactive_state: Arc<tokio::sync::Mutex<coco_compact::ReactiveCompactState>>,
    /// Auto compaction circuit breaker. Manual compaction is excluded;
    /// successful auto/session-memory compaction resets this state.
    pub(crate) auto_compact_state: Arc<tokio::sync::Mutex<coco_compact::ReactiveCompactState>>,
    /// Optional handle to the running-task manager — when present,
    /// `try_full_compact` snapshots running async agents and re-emits
    /// them as post-compact `task_status` attachments. `None` ⇒ no running-task
    /// awareness; the caller (CLI/SDK) wires this on construction.
    pub(crate) running_tasks: Option<Arc<coco_tasks::running::TaskManager>>,
    /// Last-compact tracker — feeds `RecompactionInfo`.
    /// `LastCompactState.turn_counter` resets to 0 each compact and bumps
    /// +1 per turn in `finalize_turn_post_tools` — read directly with no
    /// subtraction.
    pub(crate) last_compact_state: Arc<std::sync::Mutex<Option<LastCompactState>>>,
    /// Pre-rendered skill snapshot for in-band post-compact attachment.
    /// Caller (CLI/SDK runner) refreshes this whenever invoked skills
    /// change. TS calls `getInvokedSkillsForAgent()` inline; we keep it
    /// caller-driven so this crate doesn't import system-reminder types.
    pub(crate) post_compact_skills: Arc<std::sync::RwLock<Vec<coco_compact::PostCompactSkill>>>,
    /// Runtime sink for SessionStart hook side effects produced during
    /// compact context restoration. The app layer owns file watchers and
    /// other process-global effects; query only reports typed output.
    pub(crate) session_start_hook_side_effect_sink:
        Option<crate::session_start_hooks::SessionStartHookSideEffectSinkRef>,
    /// Staged context-collapse ledger. Pre-staged ranges drain into
    /// commits on PTL recovery so a 413 doesn't have to truncate the
    /// head. `None` ⇒ feature disabled (default).
    pub(crate) staged_ledger: Option<Arc<tokio::sync::Mutex<coco_compact::StagedCompactLedger>>>,
    /// Persistent session id used as the `sessionId` for any
    /// staged-collapse commit/snapshot entries. Lazily set when
    /// staged_ledger is installed.
    pub(crate) staged_session_id: uuid::Uuid,
    /// Most-recently @mentioned absolute file paths, capped at
    /// `MENTION_PRIORITY_CAPACITY`. `try_full_compact` reads this set
    /// to boost mentioned files in the post-compact restoration list
    /// (mentioned files survive even when not the most-recently-read).
    /// Self-designed augmentation; TS has no mention-aware re-injection.
    pub(crate) recently_mentioned_paths:
        Arc<tokio::sync::RwLock<std::collections::VecDeque<std::path::PathBuf>>>,
    /// One-shot `context_management` payload set by `do_reactive_compact`
    /// when the active provider supports server-side edits (Anthropic).
    /// Consumed and cleared by the next QueryParams build so the API can
    /// clear tool results in-place without breaking the prompt cache —
    /// avoiding the client-side mutation that `api_microcompact` would
    /// otherwise perform.
    pub(crate) pending_reactive_context_management:
        Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
    /// One-shot post-compaction signal. Set to `true` whenever
    /// `do_reactive_compact` / full-compaction / SM-compaction succeeds;
    /// consumed (swap-to-false) by the next `engine_turn_reminders` build.
    /// Only the immediately-following turn surfaces background task status
    /// reminders.
    pub(crate) pending_just_compacted: Arc<std::sync::atomic::AtomicBool>,
    /// Transcript writer used for marble-origami persistence and the
    /// per-turn user/assistant JSONL append. `None` disables
    /// persistence (in-memory ledger only). Caller wires this via
    /// `with_transcript_store`.
    pub(crate) transcript_store: Option<Arc<dyn coco_session::SessionStore>>,
    /// Shared session-level usage tracker. QueryEngine is rebuilt per
    /// user message, so the runtime owns this and wires it into every
    /// engine instance.
    pub(crate) session_usage_tracker: Option<Arc<tokio::sync::Mutex<CostTracker>>>,
    /// Serializes update/snapshot/write for session usage so concurrent
    /// engines cannot overwrite a newer snapshot with an older one.
    pub(crate) session_usage_write_lock: Option<Arc<tokio::sync::Mutex<()>>>,
    /// Session id (string form) used for transcript path resolution.
    /// Distinct from `staged_session_id` because TranscriptStore keys
    /// off the session id string used by the rest of the system.
    pub(crate) transcript_session_id: Option<String>,
    /// Dedup set of message UUIDs already written to the transcript
    /// JSONL during this session. Lives on `SessionRuntime` and is
    /// cloned into every per-turn engine via `with_transcript_dedup`
    /// so the same set survives across engine instances. `None`
    /// disables per-turn message persistence (only marble-origami
    /// metadata is written).
    pub(crate) transcript_dedup:
        Option<Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>>,
    /// Live message-history sink read by the periodic AgentSummary timer.
    /// Set on (background) sub-agent engines via `with_live_transcript`
    /// when the spawn enabled summarization; the engine publishes a
    /// full post-turn snapshot into it from `record_transcript_tail`.
    /// `None` for the main loop and non-summarized spawns ⇒ no snapshot
    /// is ever taken.
    pub(crate) live_transcript: Option<coco_tool_runtime::LiveTranscript>,
    /// Engine-side delivery channel for nested-memory entries surfaced
    /// by [`crate::engine_attachments::QueryEngine::drain_nested_memory_triggers`]
    /// at end-of-batch. Consumed by `engine_turn_reminders` right before
    /// building `TurnReminderInput` — the generator renders these into
    /// `<system-reminder>Contents of {path}:\n\n{content}</system-reminder>`.
    ///
    /// Populated by the trigger drain; bypasses the no-op
    /// [`crate::reminder_adapters::MemoryAdapter::nested_memories`].
    pub(crate) pending_nested_memory: std::sync::Arc<
        tokio::sync::Mutex<Vec<coco_system_reminder::generators::memory::NestedMemoryInfo>>,
    >,
    /// Session-level dedup set for nested-memory paths — once a memory
    /// file is injected in this session, subsequent file reads in the
    /// same subtree won't re-inject it. Cleared on conversation reset
    /// via
    /// [`crate::engine_attachments::QueryEngine::clear_loaded_nested_memory_paths`].
    pub(crate) loaded_nested_memory_paths:
        std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
    /// Sync hook event buffer shared with `SessionRuntime` so that
    /// SessionStart and UserPromptSubmit hook output surfaces as
    /// per-turn `hook_*` reminders. `None` disables sync-event
    /// surfacing — the orchestration layer's push then becomes a
    /// no-op. Installed on per-turn engines via
    /// [`Self::with_sync_hook_buffer`].
    pub(crate) sync_hook_buffer: Option<coco_hooks::SyncHookEventBuffer>,
}

// `new`, every `with_*` builder, and small accessor methods live in
// `crate::engine_builder`. Public entry points (`run`, `run_with_events`,
// `run_with_messages`), session lifecycle helpers
// (`emit_session_started`, `build_session_*_params`, `orchestration_ctx`),
// and the `forward_hook_events` bridge live in `crate::engine_session`.
// The compaction methods live in `crate::engine_compaction` /
// `crate::engine_finalize_turn`.

impl QueryEngine {
    /// Run the multi-turn agent loop.
    ///
    /// `cycle_turn_id` is the per-cycle wire id supplied by the
    /// runner; it's used on every `TurnEnded` emission this function
    /// makes (Completed / Interrupted / MaxTurnsReached /
    /// BudgetExhausted). `None` only when the caller didn't pass an
    /// `event_tx` — in that case no wire emit happens.
    ///
    /// Returns `(Result<QueryResult>, TokenUsage)`. The second tuple
    /// element is the accumulated token usage at the moment the
    /// loop exited — populated even on `Err` so the caller can
    /// surface partial usage on the `TurnEnded(Failed)` wire emit.
    pub(crate) async fn run_session_loop(
        &self,
        turn_messages: Vec<std::sync::Arc<Message>>,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        state_tracker: &SessionStateTracker,
        hook_tx_opt: Option<tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        history: &mut MessageHistory,
        cycle_turn_id: Option<coco_types::TurnId>,
    ) -> (Result<QueryResult, coco_error::BoxedError>, TokenUsage) {
        let mut total_usage = TokenUsage::default();
        let result = self
            .run_session_loop_inner(
                turn_messages,
                event_tx,
                state_tracker,
                hook_tx_opt,
                history,
                cycle_turn_id,
                &mut total_usage,
            )
            .await;
        (result, total_usage)
    }

    #[allow(clippy::too_many_arguments)]
    // Internal helper. Argument count is intentional — splitting into
    // a context struct adds indirection for a function with one
    // call site and would obscure that each argument has distinct
    // lifetime / ownership semantics (one `&mut`, one `&`, three
    // owned, two `Option<owned>`).
    async fn run_session_loop_inner(
        &self,
        turn_messages: Vec<std::sync::Arc<Message>>,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        state_tracker: &SessionStateTracker,
        hook_tx_opt: Option<tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        history: &mut MessageHistory,
        cycle_turn_id: Option<coco_types::TurnId>,
        total_usage: &mut TokenUsage,
    ) -> Result<QueryResult, coco_error::BoxedError> {
        // ── Loop state, grouped by lifecycle — see `engine_loop_state.rs`
        //    for the field-by-field rationale and `init_loop_state` for
        //    the bundled construction site.
        let (mut acc, mut turn_state, mut services, consts) = self
            .init_loop_state(turn_messages, &event_tx, history)
            .await;

        loop {
            if self.cancel.is_cancelled() {
                // Single writer for the user-cancel marker (dedup +
                // typed `SystemMessage::UserInterruption` + emit
                // `MessageAppended`). `in_flight_tool_calls = false`
                // here because this branch fires before any tool
                // execution started for the current turn. See
                // `engine-tui-unified-transcript-plan.md` §7.1 /
                // `history_sync::finalize_user_cancel`.
                //
                // Steering exception: on a submit-interrupt the queued user
                // message provides continuity, so skip the redundant standalone
                // marker (TS `query.ts:1046` parity). See `is_steering_interrupt`.
                if crate::history_sync::is_steering_interrupt(self.turn_abort.reason()) {
                    tracing::debug!(
                        "finalize_user_cancel: skipped standalone marker (submit-interrupt steering)"
                    );
                } else {
                    crate::history_sync::finalize_user_cancel(
                        history, /*in_flight_tool_calls*/ false, &event_tx,
                    )
                    .await;
                }
                // NOTE: we do NOT emit `TurnEnded(Interrupted)` from the
                // engine. The runner layer (`tui_runner` / `sdk_runner`)
                // owns the cancel-reason source of truth — a TUI
                // `/compact` cancel sets `SystemPreempt` in the runner's
                // `TurnAbortSignal` reason while `UserCommand::Interrupt`
                // sets `UserCancel`. Engine has no visibility into which
                // arm tripped `self.cancel`, so emitting from here would
                // force a hardcoded reason and defeat the architecture.
                // The runner reads `result.cancelled` and emits a single
                // `TurnEnded(Interrupted)` with the correct reason.
                return Ok(make_query_result(
                    &consts,
                    &acc,
                    &turn_state,
                    String::new(),
                    /*cancelled*/ true,
                    /*budget_exhausted*/ false,
                    Some("cancelled".into()),
                    history.to_vec(),
                    history.snapshot(),
                ));
            }

            // Drain the prior turn's tool-use-summary side-fork. 2s
            // hard cap; never blocks the new turn for more than that.
            // Silent no-op when no pending handle exists (first
            // iteration, or previous turn had no tool batch).
            self.drain_pending_tool_use_summary(&event_tx).await;

            let next_iteration_counts_as_turn = turn_state.count_next_iteration_as_turn
                && transition_consumes_turn(turn_state.transition.as_ref());
            turn_state.count_next_iteration_as_turn = true;
            // Budget check before each user-visible agent turn. Recovery
            // retries rebuild the same turn, so they still observe token /
            // continuation budgets but do not consume `max_turns`.
            let budget_turn = if next_iteration_counts_as_turn {
                turn_state.turn
            } else {
                -1
            };
            match turn_state.budget.check(budget_turn) {
                BudgetDecision::Stop { reason } => {
                    warn!(%reason, "budget stop");
                    let hit_max_turns = next_iteration_counts_as_turn
                        && self
                            .config
                            .max_turns
                            .is_some_and(|max| turn_state.turn >= max);
                    if hit_max_turns {
                        let payload = coco_messages::MaxTurnsReachedPayload {
                            max_turns: self.config.max_turns.unwrap_or(0),
                            turn_count: turn_state.turn,
                        };
                        acc.run_artifacts.max_turns_reached = Some(payload.clone());
                        crate::history_sync::history_push_and_emit(
                            history,
                            Message::Attachment(
                                coco_messages::AttachmentMessage::silent_max_turns_reached(payload),
                            ),
                            &event_tx,
                        )
                        .await;
                    }
                    // Wire-protocol terminator. Two outcomes: `MaxTurnsReached`
                    // when the turn budget is exhausted, `BudgetExhausted` for
                    // the generic 90%/diminishing-returns token-budget stop.
                    // Uses the runner-supplied cycle id so this pairs with
                    // the runner's TurnStarted. `budget_tokens` is the
                    // configured ceiling — `None` when no explicit max
                    // was set (the 90%-of-window heuristic still drove
                    // the stop; emitting 0 would be a lie).
                    if let Some(id) = cycle_turn_id.as_ref() {
                        let outcome_params = if hit_max_turns {
                            coco_types::TurnEndedParams::max_turns_reached(
                                id.clone(),
                                Some(*total_usage),
                                self.config.max_turns.unwrap_or(0),
                            )
                        } else {
                            coco_types::TurnEndedParams::budget_exhausted(
                                id.clone(),
                                Some(*total_usage),
                                total_usage.input_tokens.total + total_usage.output_tokens.total,
                                self.config.total_token_budget,
                            )
                        };
                        let _ = emit_protocol(
                            &event_tx,
                            crate::ServerNotification::TurnEnded(outcome_params),
                        )
                        .await;
                    }
                    let last_text = extract_last_assistant_text(history);
                    return Ok(make_query_result(
                        &consts,
                        &acc,
                        &turn_state,
                        last_text,
                        /*cancelled*/ false,
                        /*budget_exhausted*/ true,
                        Some(
                            if hit_max_turns {
                                "max_turns"
                            } else {
                                "budget_exhausted"
                            }
                            .into(),
                        ),
                        history.to_vec(),
                        history.snapshot(),
                    ));
                }
                BudgetDecision::Nudge { message } => {
                    info!(%message, "budget nudge");
                    // No direct ServerNotification for budget nudge; emit as non-retryable Error
                    // so SDK consumers can surface the warning.
                    let _delivered = emit_protocol(
                        &event_tx,
                        crate::ServerNotification::Error(coco_types::ErrorParams {
                            message,
                            category: Some("budget".into()),
                            retryable: false,
                        }),
                    )
                    .await;
                }
                BudgetDecision::Continue => {}
            }

            turn_state.attempt += 1;
            if next_iteration_counts_as_turn {
                turn_state.turn += 1;
            }
            let turn_id = format!("turn-{}", turn_state.attempt);

            self.consume_pending_plan_mode_clear_context(history, &event_tx, turn_state.turn)
                .await;

            let prepared_turn = self
                .enter_turn_and_prepare_request(
                    &consts,
                    &acc,
                    &mut turn_state,
                    &mut services,
                    history,
                    &event_tx,
                    hook_tx_opt.as_ref(),
                    cycle_turn_id.clone(),
                    &turn_id,
                )
                .await;
            let crate::engine_turn_request::PreparedTurnRequest {
                params,
                active_snapshot,
                messages_snapshot,
                streaming_ctx,
                streaming_handle,
                streaming_model_index,
            } = prepared_turn;
            let mut streaming_handle = streaming_handle;
            let mut streaming_model_index = streaming_model_index;

            match self.check_blocking_limit(
                history,
                &active_snapshot,
                &turn_state,
                params.max_tokens,
            ) {
                crate::engine_recovery::BlockingLimitDecision::Block {
                    estimated_tokens,
                    context_window,
                } => {
                    return Ok(self
                        .handle_blocking_limit_terminal(
                            &consts,
                            &acc,
                            &turn_state,
                            &active_snapshot,
                            estimated_tokens,
                            context_window,
                            history,
                            &event_tx,
                            cycle_turn_id.clone(),
                            &*total_usage,
                        )
                        .await);
                }
                crate::engine_recovery::BlockingLimitDecision::Proceed => {}
            }

            // Pre-API image-size guard (TS `validateImagesForAPI`): reject an
            // oversized image before the wire with a clear "please resize" error
            // instead of a raw provider 400. Terminal — the image can't be
            // auto-shrunk here.
            if let Err(img_err) = coco_messages::validate_images_for_api(
                &params.prompt,
                coco_config::constants::API_IMAGE_MAX_BASE64_SIZE as usize,
            ) {
                return Ok(self
                    .handle_image_too_large_terminal(
                        &consts,
                        &acc,
                        &turn_state,
                        &img_err,
                        history,
                        &event_tx,
                        cycle_turn_id.clone(),
                        &*total_usage,
                    )
                    .await);
            }

            let api_start = std::time::Instant::now();
            let opened_stream = match self
                .open_turn_stream(
                    &active_snapshot,
                    &params,
                    &mut services,
                    &mut turn_state,
                    &mut *history,
                    &event_tx,
                    &turn_id,
                )
                .await
            {
                Ok(opened) => opened,
                Err(crate::engine_recovery::StreamErrorOutcome::Continue) => continue,
                Err(crate::engine_recovery::StreamErrorOutcome::Bail(err)) => return Err(err),
            };
            let crate::engine_recovery::OpenedTurnStream {
                mut rx,
                token: stream_token,
                snapshot: opened_runtime_snapshot,
            } = opened_stream;

            let consumed = self
                .consume_stream(
                    &mut rx,
                    &event_tx,
                    &mut *history,
                    hook_tx_opt.as_ref(),
                    &mut streaming_handle,
                    streaming_ctx.as_ref(),
                    &mut streaming_model_index,
                    state_tracker,
                    &turn_id,
                    &consts,
                    &services,
                    &mut acc,
                    &turn_state,
                )
                .await;
            let crate::engine_stream_consume::StreamConsumed {
                response_text,
                tool_order,
                tool_buffers,
                outcome,
            } = consumed;

            let api_elapsed_ms = api_start.elapsed().as_millis() as i64;
            acc.api_time_ms += api_elapsed_ms;

            if self.cancel.is_cancelled() {
                self.model_runtimes.finish_call(
                    &stream_token,
                    coco_inference::ModelCommunicationOutcome::Failure,
                );
                self.cancel_epilogue(
                    &mut streaming_handle,
                    &tool_order,
                    &tool_buffers,
                    &mut *history,
                    &event_tx,
                    &services,
                    &consts,
                )
                .await;
                continue;
            }

            let (snapshot, usage, parsed_stop_reason) = match outcome {
                crate::engine_stream_consume::StreamOutcome::Errored {
                    message,
                    had_output,
                } => {
                    match self
                        .handle_stream_error(
                            message,
                            had_output,
                            &mut services,
                            &stream_token,
                            &mut turn_state,
                            &mut *history,
                            &event_tx,
                            &mut streaming_handle,
                            &turn_id,
                        )
                        .await
                    {
                        crate::engine_recovery::StreamErrorOutcome::Continue => continue,
                        crate::engine_recovery::StreamErrorOutcome::Bail(err) => return Err(err),
                    }
                }
                crate::engine_stream_consume::StreamOutcome::Finished {
                    snapshot,
                    usage,
                    stop_reason,
                } => (snapshot, usage, Some(stop_reason)),
                crate::engine_stream_consume::StreamOutcome::PrematureClose => match self
                    .handle_stream_error(
                        "LLM stream closed before finish event".to_string(),
                        // Conservative: a premature close carries no
                        // content signal and never classifies as a
                        // capacity error, so in-place retry is moot —
                        // pass `true` to keep this path's behavior fixed.
                        /*had_output*/
                        true,
                        &mut services,
                        &stream_token,
                        &mut turn_state,
                        &mut *history,
                        &event_tx,
                        &mut streaming_handle,
                        &turn_id,
                    )
                    .await
                {
                    crate::engine_recovery::StreamErrorOutcome::Continue => continue,
                    crate::engine_recovery::StreamErrorOutcome::Bail(err) => return Err(err),
                },
            };

            self.model_runtimes.finish_call(
                &stream_token,
                coco_inference::ModelCommunicationOutcome::Success,
            );
            // The stream reached a non-error terminal — replenish the
            // mid-stream capacity retry budget for the next turn.
            turn_state.stream_capacity_retries = 0;
            if let Some(app_state) = self.app_state.as_ref() {
                crate::engine_helpers::clear_rate_limit_observation(
                    app_state,
                    &opened_runtime_snapshot.provider,
                )
                .await;
            }

            acc.total_usage += usage;
            *total_usage = acc.total_usage;
            turn_state.budget.record_usage(&usage);
            let provider = opened_runtime_snapshot.provider.clone();
            let model_id = opened_runtime_snapshot.model_id.clone();
            acc.cost_tracker
                .record_usage(&provider, &model_id, usage, api_elapsed_ms);
            self.record_session_usage(&event_tx, &provider, &model_id, usage, api_elapsed_ms)
                .await;

            self.stamp_assistant_now();

            let (content_parts, tool_calls) = assistant_content_from_snapshot(
                &snapshot,
                crate::tool_input_normalizer::ToolInputNormalizationContext { cwd: None },
            );
            let assistant_msg = Message::Assistant(coco_messages::AssistantMessage {
                message: LlmMessage::Assistant {
                    content: content_parts
                        .into_iter()
                        .map(convert_to_assistant_content)
                        .collect(),
                    provider_options: None,
                },
                uuid: uuid::Uuid::new_v4(),
                model: model_id.clone(),
                stop_reason: parsed_stop_reason,
                usage: Some(usage),
                cost_usd: None,
                // Streaming-path response.id is not currently plumbed
                // through `StreamEvent::Finish`. The marker uses the
                // anchor index (set by `MessageHistory::anchor_api_response`
                // after this push), so `request_id` is purely for trace
                // diagnostics today — `None` here is non-load-bearing.
                request_id: None,
                api_error: None,
            });

            let withheld_opt = if tool_calls.is_empty() {
                parsed_stop_reason.and_then(crate::engine_stream_consume::withhold_reason_for_stop)
            } else {
                None
            };

            if let Some(withheld) = withheld_opt {
                match self
                    .run_post_stream_recovery(
                        withheld,
                        assistant_msg,
                        &mut *history,
                        &event_tx,
                        &mut turn_state,
                        &opened_runtime_snapshot,
                    )
                    .await
                {
                    crate::engine_recovery::RecoveryDisposition::Continue(transition) => {
                        turn_state.transition = Some(transition);
                        continue;
                    }
                    crate::engine_recovery::RecoveryDisposition::TerminateExhausted => {}
                }
            } else if tool_calls.is_empty()
                && parsed_stop_reason == Some(coco_messages::StopReason::ContentFilter)
            {
                warn!(
                    turn = turn_state.turn,
                    turn_id = %turn_id,
                    "content-filter / refusal — emitting api_error message and ending turn"
                );
                crate::history_sync::history_push_and_emit(history, assistant_msg, &event_tx).await;
                crate::history_sync::history_push_and_emit(
                    history,
                    crate::helpers::build_abnormal_stop_api_error_message(
                        coco_messages::StopReason::ContentFilter,
                        crate::engine_recovery::effective_max_tokens(
                            &opened_runtime_snapshot,
                            &turn_state,
                        ),
                    ),
                    &event_tx,
                )
                .await;
            } else {
                crate::history_sync::history_push_assistant_with_usage_and_emit(
                    history,
                    assistant_msg,
                    usage,
                    coco_types::ProviderModelSelection {
                        provider: provider.clone(),
                        model_id: model_id.clone(),
                    },
                    &event_tx,
                )
                .await;
            }

            if let Some(max_budget_usd) = self.config.max_budget_usd {
                let total_cost_usd = acc.cost_tracker.total_cost_usd();
                if total_cost_usd >= max_budget_usd {
                    warn!(total_cost_usd, max_budget_usd, "maximum USD budget reached");
                    if let Some(handle) = streaming_handle.take()
                        && streaming_ctx.is_some()
                    {
                        crate::engine_tool_commit::commit_streaming_tool_outcomes(
                            handle,
                            crate::engine_tool_commit::StreamingCommitMode::TerminalDrain,
                            history,
                            &event_tx,
                            &mut acc.run_artifacts,
                        )
                        .await;
                    }
                    return Ok(self
                        .handle_usd_budget_terminal(
                            &consts,
                            &acc,
                            &turn_state,
                            response_text,
                            history,
                            &event_tx,
                            cycle_turn_id,
                            &*total_usage,
                            &tool_calls,
                            total_cost_usd,
                            max_budget_usd,
                        )
                        .await);
                }
            }

            let streaming_executed = streaming_ctx.is_some() && !tool_calls.is_empty();
            let mut streaming_control_prevent: Option<String> = None;
            if let Some(handle) = streaming_handle.take()
                && streaming_executed
            {
                streaming_control_prevent =
                    crate::engine_tool_commit::commit_streaming_tool_outcomes(
                        handle,
                        crate::engine_tool_commit::StreamingCommitMode::CommitFlush,
                        history,
                        &event_tx,
                        &mut acc.run_artifacts,
                    )
                    .await
                    .prevent_continuation;
            }

            if tool_calls.is_empty() {
                match self
                    .handle_no_tool_calls_terminal(
                        &consts,
                        &mut acc,
                        &mut turn_state,
                        response_text,
                        history,
                        &event_tx,
                        hook_tx_opt.as_ref(),
                        cycle_turn_id.clone(),
                        usage,
                        parsed_stop_reason,
                    )
                    .await
                {
                    crate::engine_terminal::NoToolCallsTerminal::ContinueLoop => {
                        continue;
                    }
                    crate::engine_terminal::NoToolCallsTerminal::Return(result) => {
                        return Ok(*result);
                    }
                }
            }

            match self
                .execute_or_finalize_tool_calls(
                    &consts,
                    &mut acc,
                    &mut turn_state,
                    response_text,
                    history,
                    &event_tx,
                    hook_tx_opt.as_ref(),
                    state_tracker,
                    &services,
                    cycle_turn_id.clone(),
                    usage,
                    parsed_stop_reason,
                    &tool_calls,
                    messages_snapshot.clone(),
                    &opened_runtime_snapshot,
                    streaming_ctx.clone(),
                    streaming_executed,
                    streaming_control_prevent,
                )
                .await
            {
                crate::engine_tool_execution::ToolExecutionBranch::ContinueLoop => {
                    continue;
                }
                crate::engine_tool_execution::ToolExecutionBranch::Return(result) => {
                    return Ok(*result);
                }
            }
        }
    }

    async fn consume_pending_plan_mode_clear_context(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        turn: i32,
    ) {
        consume_pending_plan_mode_clear_context(self.app_state.as_ref(), history, event_tx, turn)
            .await;
    }
}

// Other impl blocks for `QueryEngine` live in:
// - `crate::engine_builder` — `new()` + every `with_*` builder + accessors;
// - `crate::engine_session` — public entry points + lifecycle + hook forwarder;
// - `crate::engine_compaction` — `try_full_compact` / SM / `run_manual_compact`;
// - `crate::engine_finalize_turn` — tail-of-turn compact ladder + reactive recovery;
// - `crate::engine_prompt` — `build_prompt` / tool-defs / context factory / date-change;
// - `crate::engine_turn_reminders` — per-turn reminder pipeline.

/// Rebuild the assistant `Vec<AssistantContentPart>` plus the
/// `Vec<ToolCallPart>` view of completed tool calls from a per-turn
/// snapshot produced by `coco-inference::process_stream_with_config`.
///
/// Walks `snapshot.parts` in emission order so the resulting vector
/// preserves the original text↔reasoning↔tool sequence (matters for
/// Gemini-3, which interleaves freely). Each part carries its own
/// `provider_metadata` — signatures and equivalents round-trip verbatim.
///
/// Tool-call filter: include when `is_input_complete || is_complete`.
/// Some providers (and the synthetic mock helper) only emit
/// `ToolInputStart`/`Delta`/`End` without the canonical `ToolCall(tc)`
/// close, so requiring `is_complete` would silently drop valid tool
/// calls. Malformed JSON is skipped with a `tracing::warn!`,
/// matching the previous behavior on the buffer-driven reconstruction
/// path.
pub(crate) fn assistant_content_from_snapshot(
    snapshot: &coco_inference::AssistantTurnSnapshot,
    normalizer_ctx: crate::tool_input_normalizer::ToolInputNormalizationContext<'_>,
) -> (Vec<AssistantContentPart>, Vec<ToolCallPart>) {
    let mut content_parts: Vec<AssistantContentPart> = Vec::with_capacity(snapshot.parts.len());
    let mut tool_calls: Vec<ToolCallPart> = Vec::new();

    for part in &snapshot.parts {
        match part {
            coco_inference::TurnPart::Text(t) => {
                if t.text.is_empty() {
                    continue;
                }
                content_parts.push(AssistantContentPart::Text(TextPart {
                    text: t.text.clone(),
                    provider_metadata: t.provider_metadata.clone(),
                }));
            }
            coco_inference::TurnPart::Reasoning(r) => {
                // Drop only TRULY empty segments. A segment with empty text but
                // non-None `provider_metadata` carries a chain-of-thought
                // carrier (OpenAI `encryptedContent`, Anthropic `signature`,
                // Gemini `thoughtSignature`) that MUST round-trip into history,
                // or store=false reasoning continuity breaks every turn.
                if r.text.is_empty() && r.provider_metadata.is_none() {
                    continue;
                }
                content_parts.push(AssistantContentPart::Reasoning(ReasoningPart {
                    text: r.text.clone(),
                    provider_metadata: r.provider_metadata.clone(),
                }));
            }
            coco_inference::TurnPart::ToolCall(tc) => {
                if !(tc.is_input_complete || tc.is_complete) {
                    warn!(tool_call_id = %tc.id, "tool call did not complete");
                    continue;
                }
                // Parse with repair, falling back to `Value::Object({})`
                // on failure. See streaming-path commentary above —
                // schema validation reports specific missing
                // fields rather than a generic "JSON broken", and the
                // tool_use/tool_result pairing invariant is preserved
                // because we never drop the call.
                let parsed_input = crate::tool_input_parse::parse_tool_arguments_or_empty(
                    &tc.input_json,
                    &tc.tool_name,
                );
                let input = crate::tool_input_normalizer::normalize_observable_tool_input(
                    &tc.tool_name,
                    parsed_input,
                    normalizer_ctx,
                );
                // Carry the wire-level `invalid` + `invalid_reason`
                // through reconstruction. Provider adapters set these
                // when wire parsing detects an unrecoverable JSON parse
                // (Anthropic streaming `content_block_stop` flush,
                // etc.); without this carry-through the agent loop's
                // synthetic `<tool_use_error>` wrap would fall back to
                // a generic message.
                let tcp = ToolCallPart {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.tool_name.clone(),
                    input,
                    provider_executed: tc.provider_executed,
                    invalid: tc.invalid,
                    invalid_reason: tc.invalid_reason.clone(),
                    provider_metadata: tc.provider_metadata.clone(),
                };
                content_parts.push(AssistantContentPart::ToolCall(tcp.clone()));
                tool_calls.push(tcp);
            }
            // File / ReasoningFile / Source / Custom / ToolApprovalRequest
            // are not yet round-tripped through assistant history. Adding
            // them here is a follow-up; for now they're emission-only via
            // the live UI stream events. Drop with a trace.
            other => {
                tracing::trace!(?other, "snapshot variant not yet reconstructed");
            }
        }
    }

    (content_parts, tool_calls)
}

async fn consume_pending_plan_mode_clear_context(
    app_state: Option<&Arc<RwLock<ToolAppState>>>,
    history: &mut MessageHistory,
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    turn: i32,
) {
    let Some(state_handle) = app_state else {
        return;
    };
    let (clear_history, implementation_message) = {
        let mut w = state_handle.write().await;
        (
            std::mem::take(&mut w.pending_clear_message_history),
            w.pending_plan_implementation_message.take(),
        )
    };
    if !clear_history {
        return;
    }

    // I-1 (Authority): every transcript mutation must emit so TUI's
    // TranscriptView + SDK NDJSON observers stay coherent. Plan-mode
    // exit doesn't rotate session_id, so `MessageTruncated { 0 }` is
    // the right signal; the following user message carries the
    // implementation message content.
    crate::history_sync::history_clear_and_emit(history, event_tx).await;
    if let Some(message) = implementation_message {
        crate::history_sync::history_push_and_emit(
            history,
            create_plan_implementation_message(&message),
            event_tx,
        )
        .await;
    }
    info!(turn, "plan-mode exit cleared conversation history");
}

/// Per-run side-channel collectors filled at emission sites so finalize
/// doesn't need to scan history.
///
/// `structured_output` is set when a tool returns `with_structured_output(...)`
/// (via `UnstampedToolCallOutcome.structured_output`). `max_turns_reached` is
/// set when the engine hits the configured `max_turns` cap. Both feed
/// `QueryResult` — and from there `SessionResultParams` — without re-deriving
/// state from `history`, which mid-run compaction can replace.
#[derive(Default, Clone)]
pub(crate) struct RunArtifacts {
    pub structured_output: Option<serde_json::Value>,
    pub max_turns_reached: Option<coco_messages::MaxTurnsReachedPayload>,
    /// Count of `StructuredOutput` tool invocations made by the model
    /// during this query (successful + failed). Used to enforce
    /// `COCO_MAX_STRUCTURED_OUTPUT_RETRIES` and to decide whether to
    /// re-inject the "you MUST call this tool" nudge when the model
    /// tries to end the turn without a successful structured response.
    pub structured_output_attempts: u32,
}

fn transition_consumes_turn(transition: Option<&ContinueReason>) -> bool {
    matches!(transition, None | Some(ContinueReason::NextTurn))
}

#[cfg(test)]
#[path = "engine.test.rs"]
mod tests;

#[cfg(test)]
#[path = "engine_live_rules_scoping.test.rs"]
mod engine_live_rules_scoping_tests;
