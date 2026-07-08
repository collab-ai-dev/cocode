//! Per-session runtime container shared by both TUI and SDK runners.
//!
//! The TUI runner (`tui_runner::run_tui` / `run_agent_driver`) and the SDK
//! runner (`sdk_server::sdk_runner::QueryEngineRunner`) both need to:
//!
//! 1. Construct ~12 per-session subsystem state objects at startup
//! (`FileReadState`, `SessionMemoryService`, `HookRegistry`,
//! `CompactionObserverRegistry`, `FileHistoryState`, `ToolAppState`,
//! history Mutex, …).
//! 2. Per-turn, build a `QueryEngine` by chaining ~11 `.with_*` calls
//! that install those subsystems on the engine.
//! 3. On `/clear`, perform a full reset (SessionEnd hooks → drop
//! caches → regen session id → SessionStart hooks).
//!
//! Before this module existed, both runners had their own copies of
//! steps 1+2+3 — the SDK copy had drifted to ~30% completeness and 7
//! distinct bugs that all had the same shape ("TUI installed X, SDK
//! forgot to install X"). [`SessionRuntime`] is the single owner of
//! that state; both runners construct one at startup, then call
//! [`SessionRuntime::build_engine`] per turn and
//! [`SessionRuntime::clear_conversation`] on `/clear`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::Mutex;
use tokio::sync::RwLock;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_context::FileHistoryState;
use coco_context::FileReadState;
use coco_hooks::HookRegistry;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_query::CommandQueue;
use coco_query::QueryEngineConfig;
use coco_session::SessionManager;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::MailboxHandleRef;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::PermissionMode;
use coco_types::SessionId;
use coco_types::ToolAppState;
use tokio_util::sync::CancellationToken;

use crate::Cli;
use crate::process_runtime::ProcessRuntime;
use crate::project_services::ProjectServices;

mod agent_catalog;
mod build;
mod clear;
mod engine;
mod handles;
mod hooks;
mod permissions;
mod reload;
mod retarget;
mod roles;
mod sandbox;
mod session_handle;
mod state;

pub(crate) use permissions::live_permissions;
pub use roles::RoleOverride;
pub(crate) use roles::resolve_model_selection_from_runtime_config;
#[cfg(test)]
pub(crate) use roles::thinking_level_for_effort_from;
pub(crate) use sandbox::build_sandbox_state;
pub(crate) use sandbox::sandbox_settings_deny_paths;
pub use session_handle::SessionHandle;

fn clone_std_rwlock<T: Clone>(lock: &std::sync::RwLock<T>) -> T {
    match lock.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn write_std_rwlock<T>(lock: &std::sync::RwLock<T>, value: T) {
    match lock.write() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

/// Options for building a [`SessionRuntime`].
pub struct SessionRuntimeBuildOpts<'a> {
    pub cli: &'a Cli,
    pub runtime_config: Arc<RuntimeConfig>,
    pub cwd: PathBuf,
    pub model_id: String,
    pub system_prompt: String,
    pub permission_mode_availability: coco_types::PermissionModeAvailability,
    pub permission_mode: PermissionMode,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub tools: Arc<ToolRegistry>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    /// SDK runner installs an `SdkPermissionBridge`; TUI passes `None`
    /// and uses interactive approval prompts instead.
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    /// Slash-command registry — populated once at startup via
    /// `coco_commands::build_command_registry`. Both typed `/foo`
    /// dispatch and command-palette execution snapshot this registry
    /// before sending model-bound follow-ups through AppServer turn/start.
    /// Wrapped in `RwLock` so `/reload-plugins` can rebuild and swap
    /// without restarting the session — consumers snapshot the inner
    /// `Arc<CommandRegistry>` once per dispatch via
    /// [`SessionRuntime::current_command_registry`].
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped `SkillManager` — same Arc that backed
    /// `command_registry`'s skill load, kept alive so the per-turn
    /// reminder pipeline (`SkillsSource`) reads the same catalog.
    pub skill_manager: Arc<coco_skills::SkillManager>,
    /// Project-scoped services/catalog loaded for this session's project root.
    pub project_services: Arc<ProjectServices>,
    /// Process-scoped owner used for project-service reloads during this
    /// session's lifetime.
    pub process_runtime: Arc<ProcessRuntime>,
    /// Where to look for markdown agent definitions. Threaded into the
    /// runtime's [`coco_subagent::AgentDefinitionStore`] so AgentTool's
    /// dynamic prompt sees the same set the SDK `initialize.agents`
    /// listing reports. Empty = no on-disk agents (built-ins only).
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
    /// Built-in catalog toggles. Defaults to [`coco_subagent::BuiltinAgentCatalog::interactive`]
    /// (CLI / TUI sessions); SDK noninteractive callers may pass
    /// [`coco_subagent::BuiltinAgentCatalog::sdk_noninteractive`] to
    /// disable the entire built-in roster.
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Session id to adopt (resume / continue / fork). `None` mints a
    /// fresh per-process uuid. Threaded so every runtime subsystem
    /// (task dirs, task-list id, agent transcripts, usage snapshot)
    /// keys off the SAME id the engine config uses.
    pub session_id_override: Option<SessionId>,
    /// True for SDK / headless (print) sessions. File-history checkpointing
    /// defaults OFF for these and ON for the interactive TUI, unless
    /// overridden by `COCO_FILE_CHECKPOINTING_*`.
    pub is_non_interactive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnginePersistenceMode {
    MainSession,
    Fork,
}

/// Shared handle to a `QueryEngine`'s post-turn cache-safe-params slot, as
/// returned by `QueryEngine::cache_safe_params_handle`. Kept as an alias so
/// the runtime field that stores the latest one stays readable.
type CacheParamsHandle = Arc<RwLock<Option<coco_types::CacheSafeParams>>>;

/// All per-session state shared by both runners. Construction at startup
/// is done once via [`SessionRuntime::build`]; per-turn engines are
/// assembled via [`SessionRuntime::build_engine`].
pub struct SessionRuntime {
    // ── immutable resources (never change after build) ─────────────────
    /// Tool registry shared by every engine instance. Read by
    /// [`Self::build_engine`] / [`Self::build_engine_from_config`].
    tools: Arc<ToolRegistry>,
    /// Slash-command registry. Read by
    /// [`crate::tui_runner::dispatch_slash_command`] to resolve every
    /// `/foo` typed by the user or selected from the command palette.
    /// Wrapped in `RwLock` so `/reload-plugins` can rebuild and swap
    /// without restarting the session — consumers snapshot the inner
    /// `Arc<CommandRegistry>` once per dispatch via
    /// [`Self::current_command_registry`] so a concurrent swap can't
    /// invalidate borrows.
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped skill catalog. Cloned into `ReminderSources`
    /// (`SkillsSource`) on every per-turn engine so the model receives
    /// the `skill_listing` reminder that gates on
    /// `skill_manager.is_empty()`.
    pub(crate) skill_manager: Arc<coco_skills::SkillManager>,
    pub config_home: PathBuf,
    pub runtime_config: Arc<RuntimeConfig>,
    pub process_runtime: Arc<ProcessRuntime>,
    pub project_services: Arc<ProjectServices>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    schedule_store: coco_tool_runtime::ScheduleStoreRef,
    model_runtimes: Arc<coco_inference::ModelRuntimeRegistry>,
    side_query: coco_tool_runtime::SideQueryHandle,
    usage_accounting: coco_query::usage_accounting::UsageAccounting,
    pub auto_title_enabled: bool,
    /// SwarmMailbox handle installed on every engine via `with_mailbox`.
    mailbox: MailboxHandleRef,
    /// Optional SDK permission bridge (None for TUI). Installed via
    /// `with_permission_bridge` when present.
    permission_bridge: Option<ToolPermissionBridgeRef>,
    /// Long-lived parent token for runtime-level lifecycle (hook
    /// orchestration shutdown). Per-turn engine cancels are
    /// independent — see TUI `run_agent_driver` for per-iteration
    /// `CancellationToken::new()`.
    cancel: CancellationToken,

    /// Original CWD captured at session start. Frozen for the lifetime
    /// of this [`SessionRuntime`] — never moves even if the user
    /// `cd`'s away inside a Bash command. Used as the anchor for
    /// `reset_cwd_if_outside_project` (when bash drifts out of the
    /// allowed working directory set, we snap it back here) and for
    /// "Shell cwd was reset to …" stderr annotations.
    pub original_cwd: PathBuf,
    /// Git worktree root for project-scoped services, or
    /// [`Self::original_cwd`] when the session is outside git.
    pub project_root: PathBuf,
    /// Existing session storage layout anchor. This intentionally remains
    /// separate from [`Self::project_root`] until transcript storage is
    /// migrated to the ProjectServices root.
    pub project_paths: Arc<coco_paths::ProjectPaths>,

    // ── mutable per-session state (changes on /clear or mid-session) ──
    /// Currently active CWD. Updated **across BashTool calls** so the
    /// model's `cd /tmp` in one turn survives into the next turn.
    /// Threaded into every `ToolUseContext` via the engine config so
    /// BashTool can read it as the spawn cwd and write back from
    /// `CommandResult.new_cwd`.
    pub current_cwd: Arc<RwLock<PathBuf>>,
    /// Engine config; mutated by [`Self::clear_conversation`] (session_id)
    /// and [`Self::update_engine_config`]. Read by every per-turn build.
    engine_config: Arc<RwLock<QueryEngineConfig>>,
    /// Synchronous snapshot for detached hook factories. Those
    /// factories run from async tasks but expose a sync `Fn()`, so they
    /// must not call Tokio `blocking_read()` on runtime worker threads.
    orchestration_engine_config: Arc<std::sync::RwLock<QueryEngineConfig>>,
    /// Per-session in-memory model-role overrides. Populated by the TUI
    /// model picker (`UserCommand::SetModelRole`) and Ctrl+T thinking
    /// cycle (`UserCommand::SetThinkingLevel`). Layered ABOVE
    /// `runtime_config.model_roles` — [`Self::resolve_role`] checks
    /// overrides first, falls back to the runtime config map second.
    /// **Not persisted.** Model-role changes via the TUI are session-local;
    /// users who want a binding to survive across sessions edit
    /// `the global config file::model_roles.<role>.primary` themselves.
    /// Cleared on `Drop` (i.e. session end) via the natural `Arc`
    /// lifecycle. `/clear` keeps overrides — the conversation reset is
    /// orthogonal to model-role bindings.
    role_overrides: Arc<RwLock<HashMap<ModelRole, RoleOverride>>>,
    pub file_read_state: Arc<RwLock<FileReadState>>,
    pub file_history: Option<Arc<RwLock<FileHistoryState>>>,
    pub app_state: Arc<RwLock<ToolAppState>>,
    /// `/loop` scheduled sentinel memory. Reset after compaction so the next
    /// sentinel delivery re-establishes full instructions in the transcript.
    pub loop_sentinel_state: Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>>,
    /// Session-scoped peer-message store, shared (one `Arc`) by every
    /// per-turn engine built via `wire_engine` — including in-process
    /// teammate engines. `SendMessage` pushes into it (`ToolUseContext.
    /// pending_messages`) and the recipient drains it via the
    /// `agent_pending_messages` system-reminder (`SwarmAdapter`). The two
    /// sites MUST share this exact `Arc`, else messages vanish.
    pending_message_store: coco_tool_runtime::PendingMessageStoreRef,
    /// Session-scoped Auto mode classifier state. Installed on every
    /// per-turn engine so `permission_mode = Auto` can auto-approve
    /// safe/read-only tools before falling back to interactive approval.
    auto_mode_state: Arc<coco_permissions::AutoModeState>,
    /// Denial history for Auto mode classifier decisions. Shared across
    /// per-turn engines and cleared when the session changes or compacts.
    denial_tracker: Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
    /// Auto-memory runtime — extraction / dream / 9-section session
    /// memory / recall ranker. `None` when `Feature::AutoMemory` is
    /// off; otherwise threaded into every engine via
    /// [`coco_query::QueryEngine::with_memory_runtime`].
    memory_runtime: Option<Arc<coco_memory::MemoryRuntime>>,
    /// Skill-learning review runtime. `Some` when `Feature::SkillLearning` is
    /// enabled; threaded into every engine via
    /// [`coco_query::QueryEngine::with_skill_review_runtime`]. The real agent
    /// handle is late-bound in `attach_agent_handle`, same as memory.
    skill_review_runtime: Option<Arc<coco_skill_learn::SkillReviewRuntime>>,
    /// Real `AgentHandle` for `AgentTool` calls and forked subagents.
    /// Constructed once at session start, installed on every engine
    /// via `wire_engine`. `send_message`, team mgmt, async-launched
    /// agent ops work; sync subagent spawns work once the engine
    /// factory is wired (separately).
    swarm_agent_handle: coco_tool_runtime::AgentHandleRef,
    /// Hook registry merged from settings + plugin manifests. Installed
    /// on every engine + driven by SessionStart / SessionEnd in
    /// [`Self::clear_conversation`].
    pub(crate) hook_registry: Arc<HookRegistry>,
    /// LLM-driven hook handler — implements
    /// [`coco_hooks::HookLlmHandle`] for `Prompt` (full impl) and
    /// `Agent` (stub returning Cancelled — silent fallback) hook
    /// handlers. Threaded into every `OrchestrationContext` so settings
    /// hooks of `type: "prompt"` / `type: "agent"` actually reach an
    /// LLM instead of falling back to passthrough text.
    pub(crate) hook_llm_handle: Arc<coco_query::hook_llm::QueryHookLlm>,
    /// Shared sync-hook-event buffer. SessionStart and UserPromptSubmit
    /// orchestration calls push `HookEvent`s here; the
    /// [`coco_hooks::reminder_source::CombinedHookEventsSource`]
    /// installed on every per-turn engine drains them into the
    /// reminder pipeline. Lifetime spans the whole session — same
    /// instance flows through `OrchestrationContext.sync_event_sink`
    /// and `QueryEngine::sync_hook_buffer`.
    pub(crate) sync_hook_buffer: coco_hooks::SyncHookEventBuffer,
    /// Async hook bookkeeping. Currently no production code path
    /// registers async hooks, but the slot is wired into the combined
    /// reminder source so when async hook execution lands it surfaces
    /// `async_hook_response` reminders without further plumbing.
    pub(crate) async_hook_registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    /// FileChanged hook watcher. Populated when the runtime's hook
    /// registry has any handlers for the `FileChanged` event;
    /// `None` otherwise. Paths are registered lazily from
    /// `SessionStart` / `CwdChanged` hook output.
    pub(crate) file_changed_watcher:
        Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>>,
    /// Multi-turn agent transcript. Each turn snapshots, appends, and
    /// rewrites this on success. Wrapped in `MessageHistory` (the same
    /// type the engine loop uses internally) so TUI-initiated pushes
    /// can call `history_push_and_emit` directly without converting at
    /// the lock boundary.
    pub history: Arc<Mutex<MessageHistory>>,
    /// Agent-spawn handle used by `AgentTool` / coordinator-mode
    /// workers. Late-bound after `TaskRuntime` is attached because
    /// `SwarmAgentHandle` requires the canonical TaskManager-backed
    /// registry at construction.
    agent_handle: Arc<RwLock<Option<AgentHandleRef>>>,
    /// Skill-execution handle (`QuerySkillRuntime`). Late-bound for the
    /// same Arc-cycle reason as `agent_handle`: the real impl wraps the
    /// subagent `AgentQueryEngineRef` (built in `agent_handle_factory`).
    /// Installed on every per-turn engine via `wire_engine` so the model's
    /// `SkillTool` resolves; `None` ⇒ engine falls back to
    /// `NoOpSkillHandle` (every skill call returns `Unavailable`).
    skill_handle: Arc<RwLock<Option<coco_tool_runtime::SkillHandleRef>>>,
    /// Shared, per-turn-refreshed Bash handle for in-prompt skill shell
    /// expansion (`` !`cmd` ``). Set on every main-session engine build
    /// (`build_engine`) with the same `SessionBashToolHandle` injected into
    /// the command registry, and read by `QuerySkillRuntime` so the
    /// model-invoked + fork-mode skill paths run identical permission-checked
    /// shell. `std::sync::RwLock` (snapshot read, no guard across await).
    skill_bash_cell:
        Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>>,
    /// Post-turn fork dispatcher (D1/D2). Same late-bind pattern as
    /// `agent_handle`: built after `build()` returns the `Arc<Self>`
    /// (the dispatcher impl captures the runtime), and installed on
    /// every per-turn engine via `wire_engine`. `None` ⇒ post-turn
    /// forks degrade to no-op (`/btw` returns a bootstrap hint,
    /// `promptSuggestion` skips). Real impl lives in
    /// `app/cli/src/fork_dispatcher.rs`.
    fork_dispatcher: Arc<RwLock<Option<coco_query::forked_agent::ForkDispatcherRef>>>,
    /// Latest per-turn engine's cache-safe-params handle, captured on every
    /// `build_engine`. The `QueryEngine` is rebuilt each turn, but this `Arc`
    /// is shared with the engine's slot, so it keeps observing the params the
    /// engine writes at turn finalize and survives the engine drop. A
    /// between-turns `/btw` reads it to share the parent turn's prompt cache
    /// when available; otherwise it rebuilds cache params from the current
    /// transcript.
    /// `None` until the first engine is built; the inner `Option` is `None`
    /// until the first turn finalises (or after `/clear`).
    last_engine_cache_handle: Arc<RwLock<Option<CacheParamsHandle>>>,
    /// Teammate-scoped live permission-rule overlay, injected onto every
    /// main-session engine's `QueryEngineConfig.live_permission_rules` (which
    /// `ToolContextFactory` merges each batch, post-derivation).
    /// Since the main-session permission base now lives in `ToolAppState`
    /// (read-through + mutated by `apply_permission_updates_everywhere`), this
    /// overlay is NO LONGER the main-session in-cycle mechanism. It survives as
    /// the channel an in-process teammate uses for leader-pushed
    /// `TeamPermissionUpdate` rules (cross-process teammates use the mailbox →
    /// `runner_loop` `team_permission_rules` analog). Empty for a plain main
    /// session.
    live_permission_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>>,
    /// Session-scoped abort token for the in-flight prompt-suggestion
    /// fork. When a new suggestion fork starts, we cancel the previous
    /// one so users rapidly cycling `/clear` don't accumulate fork tasks
    /// burning tokens. `None` ⇒ no fork in flight.
    pub current_suggestion_abort:
        Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    /// Background task runtime (TaskHandle implementation) — owns
    /// the `TaskManager` + per-task control state. Shared with
    /// `SwarmAgentHandle` so AgentTool's bg path registers spawns
    /// through the same store the engine's `Task*` tools read from.
    /// `None` resolves to `NoOpTaskHandle` semantics (the task tools
    /// surface a clean "no task runtime configured" error).
    task_runtime: Arc<RwLock<Option<Arc<crate::task_runtime::TaskRuntime>>>>,
    /// Durable task-list store shared by the leader, AgentTool children,
    /// and in-process teammates.
    task_list: Arc<RwLock<Option<coco_tool_runtime::TaskListHandleRef>>>,
    team_task_list_router: Arc<RwLock<Option<coco_tool_runtime::TeamTaskListRouterRef>>>,
    /// Session-scoped V1 TodoWrite store. The engine is rebuilt every turn, so
    /// keeping this handle on the runtime preserves replace-all old/new
    /// semantics across turns and lets resume seed the latest transcript state.
    todo_list: Arc<RwLock<coco_tool_runtime::TodoListHandleRef>>,
    /// Per-agent transcript / metadata store for resume support.
    /// Late-bound so CLI bootstrap can construct the impl after
    /// `SessionRuntime::build` returns. `agent_handle_factory`
    /// installs it onto the SwarmAgentHandle when wiring agent-
    /// team support.
    agent_transcript_store: Arc<RwLock<Option<coco_tool_runtime::AgentTranscriptStoreRef>>>,
    /// Main-session transcript store. JSONL writes for the user /
    /// assistant / attachment / tool_result chain land here, keyed
    /// by the live session id (rotates on `/clear`). Cloned into
    /// every per-turn engine via [`Self::wire_engine`]. Backend-agnostic
    /// (`dyn SessionStore`) so it honors the configured `session.backend`.
    transcript_store: Arc<dyn coco_session::SessionStore>,
    /// When false, all transcript / usage / file-history persistence is
    /// suppressed for this run.
    persist_session: bool,
    /// Cross-engine dedup set of message UUIDs already persisted to
    /// the JSONL transcript. Lives on the runtime (not the engine)
    /// so a fresh per-turn engine doesn't re-write history. Reset to
    /// empty by [`Self::clear_conversation`] when the session id
    /// regenerates.
    transcript_dedup: Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    /// Conversation snapshot captured immediately before `/clear`.
    /// The fresh post-clear session keeps a hidden copy so `/rewind`
    /// can recover a pre-clear prompt before any new turn is submitted.
    clear_rewind_messages: Arc<tokio::sync::Mutex<Option<Vec<Arc<Message>>>>>,
    /// True after the engine writes a terminal `/goal` success snapshot
    /// to session metadata. The next main-session turn clears it, matching
    /// the TS metadata observer lifecycle.
    terminal_goal_metadata_written: Arc<AtomicBool>,
    /// Cross-engine tool-result replacement state. QueryEngine is
    /// rebuilt per user message, so this runtime-owned state preserves
    /// Level 2 `seen_ids` / replacement strings across turns.
    tool_result_replacement_state:
        coco_tool_runtime::tool_result_storage::ContentReplacementStateRef,
    /// MCP handle installed on every per-turn engine via `wire_engine`.
    /// Late-bound so CLI bootstrap can construct the
    /// `McpManagerAdapter` (or any other McpHandle impl) after
    /// `SessionRuntime::build` returns. Without this the engine's
    /// `mcp_handle` slot stays `None` and AgentTool's prompt-time
    /// MCP filter degrades to fail-closed (hides MCP-required
    /// agents).
    mcp_handle: Arc<RwLock<Option<coco_tool_runtime::McpHandleRef>>>,
    /// The live MCP connection manager, when one was built for this session.
    /// Distinct from [`Self::mcp_handle`] (the opaque tool-facing handle): this
    /// is the concrete manager so reload paths can re-register plugin-contributed
    /// MCP servers after a `/reload-plugins` / install / delisting. `None` on
    /// entry points that don't build a manager (e.g. the TUI today, headless);
    /// reload then no-ops. Set via [`Self::attach_mcp_manager`].
    mcp_manager: Arc<RwLock<Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>>>,
    /// Monotonic "the MCP server set changed" signal, bumped by
    /// [`Self::reload_plugin_mcp_servers`]. Consumers that own MCP
    /// reconnection re-run their effect when it moves.
    mcp_reconnect_key: Arc<std::sync::atomic::AtomicU64>,
    /// Late-bind slot for the LSP handle. CLI / SDK installs a
    /// `LspManagerAdapter` here when `Feature::Lsp` is on and at
    /// least one language server is configured; `wire_engine` reads
    /// the slot at engine-build time and installs it via
    /// `with_lsp_handle`.
    lsp_handle: Arc<RwLock<Option<coco_tool_runtime::LspHandleRef>>>,
    /// Where the agent loader looks for markdown agents. Cached so
    /// `/agents reload` and the file-watcher reload paths can rebuild
    /// the snapshot without re-resolving the paths from scratch. Plugin
    /// reload refreshes this from the latest `ProjectServices` snapshot before
    /// the catalog is rebuilt.
    agent_search_paths: Arc<RwLock<coco_subagent::definition_store::AgentSearchPaths>>,
    /// Built-in agent toggles applied to every reload. Set at
    /// `SessionRuntime::build` and treated as immutable thereafter
    /// (toggling the roster mid-session would require a full restart).
    builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Active per-session agent catalog snapshot. Installed on every
    /// per-turn engine via [`Self::wire_engine`] so `AgentTool::prompt`
    /// renders the dynamic agent listing. Wrapped in `RwLock<Arc<...>>`
    /// so a future reload (file watcher or `/agents reload`) can swap
    /// the inner `Arc` without invalidating in-flight per-turn engines
    /// (each engine snapshots the inner Arc at wire time).
    /// `Arc<AgentCatalogSnapshot>` is cheap to clone.
    agent_catalog: Arc<RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>>,
    /// SDK-supplied agent definitions to inject into every fresh
    /// `AgentDefinitionStore` build (initial load + every reload).
    /// Populated by the SDK `initialize` handler via
    /// [`Self::set_sdk_supplied_agents`] when the client pushes an
    /// `initialize.agents` JSON map. Stays alive across `session/start`
    /// → `session/archive` cycles so a single SDK connection's
    /// `initialize` payload survives multiple session boundaries.
    sdk_supplied_agents: Arc<RwLock<Vec<coco_types::AgentDefinition>>>,
    /// Session-scoped sandbox state. Built once at startup via
    /// [`build_sandbox_state`] and inherited by every per-turn engine
    /// (TUI), every SDK control message handler, and every fork
    /// dispatch — so all paths share the same `Arc<SandboxState>` and
    /// hot-reloads via `update_config` are seen everywhere.
    /// `None` when sandbox is disabled.
    sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
    /// Session-scoped attachment channel. Producers outside the per-turn
    /// engine (slash commands via the TUI, future swarm / skill / hook
    /// forwarders) emit typed silent `AttachmentMessage`s through
    /// [`Self::attachment_emitter`]; the engine drains the receiver at the
    /// head of every outer-loop turn via
    /// [`coco_query::QueryEngine::drain_attachment_inbox`]. Lives across
    /// engine rebuilds so cross-turn producers see a stable handle.
    session_attachment_tx: tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
    session_attachment_rx: Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<coco_messages::AttachmentMessage>>,
    >,
    /// Session-scoped mid-turn command queue. Producers (the
    /// TUI-while-busy bridge in `tui_runner`, future task / coordinator /
    /// hook forwarders) push `QueuedCommand`s here at any time, and the
    /// per-turn `QueryEngine` consumes them via [`Self::wire_engine`]
    /// which calls [`QueryEngine::with_command_queue`]. Internally
    /// `Arc`-backed so `Clone` is cheap — every engine instance shares
    /// the same backing storage with the runtime and any other holder.
    /// Teammate messages and task notifications also flow through this
    /// queue (with `QueueOrigin::Coordinator` /
    /// `QueueOrigin::TaskNotification`) — no separate `Inbox` type;
    /// coordinator messages surface as `queued_command` attachments.
    command_queue: CommandQueue,
    /// Concurrent-sessions PID registry guard. Wraps
    /// `<config_home>/sessions/{pid}.json`; the file is created at
    /// build time and removed when this field is dropped (i.e. when
    /// the last `Arc<SessionRuntime>` reference falls). `None` when
    /// the registration was skipped (subagent context per
    /// `COCO_AGENT_ID`) or the write failed (best-effort — we
    /// `tracing::warn` and proceed without a registry entry rather
    /// than block session startup).
    _pid_registry: Option<coco_session::SessionRegistry>,
}

#[cfg(test)]
#[path = "session_runtime.test.rs"]
mod tests;
