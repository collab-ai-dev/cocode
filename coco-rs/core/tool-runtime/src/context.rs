use coco_context::FileHistoryState;
use coco_context::FileReadState;
use coco_messages::Message;
use coco_types::ActiveShellTool;
use coco_types::AgentId;
use coco_types::AgentTypeId;
use coco_types::Features;
use coco_types::ThinkingLevel;
use coco_types::ToolFilter;
use coco_types::ToolOverrides;
use coco_types::ToolPermissionContext;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use std::path::PathBuf;

use crate::agent_handle::AgentHandleRef;
use crate::cancellation::ToolAbortSignal;
use crate::cancellation::TurnAbortController;
use crate::denial_tracking::DenialTracker;
use crate::hook_handle::HookHandleRef;
use crate::lsp_handle::LspHandleRef;
use crate::mcp_handle::McpHandleRef;
use crate::permission_bridge::ToolPermissionBridgeRef;
use crate::registry::ToolRegistry;
use crate::schedule_store::ScheduleStoreRef;
use crate::side_query::SideQueryHandle;
use crate::task_handle::BackgroundTaskHandleRef;
use crate::task_list_handle::TaskListHandleRef;
use crate::task_list_handle::TeamTaskListRouterRef;
use crate::task_list_handle::TodoListHandleRef;
use crate::traits::ProgressSender;

/// Context provided to every tool execution.
///
/// Organized into logical groups.
/// Passed by reference to Tool::execute(); mutated via callback closures.
#[derive(Clone)]
pub struct ToolUseContext {
    // ── Options (from QueryEngineConfig) ──
    /// Available tools registry.
    pub tools: Arc<ToolRegistry>,
    /// Main loop model identifier.
    pub main_loop_model: String,
    /// Thinking level configuration.
    pub thinking_level: Option<ThinkingLevel>,
    /// Whether this is a non-interactive session (SDK/headless). Session-level
    /// side effects only — NOT permission decisions (use
    /// `avoid_permission_prompts` for those).
    pub is_non_interactive: bool,
    /// Whether a residual `Ask` must fail closed (Deny) because no interactive
    /// prompt is reachable (`shouldAvoidPermissionPrompts`). Distinct from
    /// `is_non_interactive` so a future consumer-backed print/SDK mode can stay
    /// non-interactive while still routing `Ask` to its `canUseTool` callback.
    pub avoid_permission_prompts: bool,
    /// Cost budget limit (USD).
    pub max_budget_usd: Option<f64>,
    /// Custom system prompt override.
    pub custom_system_prompt: Option<String>,
    /// Appended system prompt.
    pub append_system_prompt: Option<String>,
    /// Debug mode.
    pub debug: bool,
    /// Verbose mode.
    pub verbose: bool,
    /// Resolved tool runtime configuration.
    pub tool_config: coco_config::ToolConfig,
    /// Resolved sandbox runtime configuration. Tools read this for the
    /// user-facing mode + excluded-commands list. Actual enforcement
    /// (wrapping commands with bwrap/Seatbelt) is driven by
    /// [`Self::sandbox_state`].
    pub sandbox_config: coco_config::SandboxSettings,
    /// Active sandbox runtime state. `None` when sandbox is disabled or not
    /// bootstrapped (test contexts, headless runs without sandbox). Tools
    /// must consult this — not `sandbox_config` — to decide whether a
    /// command runs sandboxed and to obtain the per-command snapshot used
    /// by the shell executor.
    pub sandbox_state: Option<std::sync::Arc<coco_sandbox::SandboxState>>,
    /// Resolved memory runtime configuration.
    pub memory_config: coco_config::MemoryConfig,
    /// Resolved shell runtime configuration. Consumed by Bash tool
    /// (`ShellExecutor::new_with_config`) for shell-override + snapshot
    /// gating.
    pub shell_config: coco_config::ShellConfig,
    /// Model-facing shell tool selected for this session.
    pub active_shell_tool: ActiveShellTool,
    /// Session-scoped shell command assembler. Constructed once at
    /// session bootstrap and threaded through every tool invocation so
    /// that snapshot capture, session-env hook output, `/env` vars,
    /// and `COCO_SHELL_PREFIX` survive across calls.
    ///
    /// `None` for tests / SDK paths that haven't wired the provider —
    /// `BashTool` falls back to constructing a fresh per-call executor
    /// (no snapshot benefit but still functional).
    pub shell_provider: Option<std::sync::Arc<dyn coco_shell::ShellProvider>>,
    /// Frozen anchor — captured at session start. BashTool's
    /// `reset_cwd_if_outside_project` uses it to snap back when the
    /// live cwd drifts out of the allowed working set. `None` for
    /// tests / agent-worktree paths.
    pub original_cwd: Option<std::path::PathBuf>,
    /// Mutable session CWD shared across all BashTool invocations.
    /// `cd /tmp` in turn N updates this; turn N+1 reads it as the
    /// spawn cwd. `None` ⇒ BashTool uses `std::env::current_dir()`
    /// (per-call, no persistence — legacy / test path).
    pub session_cwd: Option<std::sync::Arc<tokio::sync::RwLock<std::path::PathBuf>>>,
    /// Resolved web-fetch runtime configuration. Consumed by the
    /// `WebFetchTool` for timeout / max-content-length / user-agent.
    pub web_fetch_config: coco_config::WebFetchConfig,
    /// Resolved web-search runtime configuration. Consumed by the
    /// `WebSearchTool` for max-results.
    pub web_search_config: coco_config::WebSearchConfig,
    /// Resolved plan-mode runtime settings. Consumed by
    /// `ExitPlanModeTool` to decide whether to surface the multi-choice
    /// clear-context dialog, and by the engine main loop to read the
    /// `plan_model_fallback_threshold_tokens` value when computing the
    /// plan-mode model swap.
    pub plan_mode_settings: coco_config::PlanModeSettings,
    /// Resolved LSP tool-layer runtime configuration. Consumed by the
    /// `LspTool` for the per-query file-size gate. Server roster lives
    /// in `coco-lsp::LspServersConfig` (separate config file) — this
    /// struct only carries cross-server tool-side knobs.
    pub lsp_config: coco_config::LspConfig,
    /// Centralized feature gates. See
    /// `docs/coco-rs/feature-gates-and-tool-filtering.md`.
    pub features: Arc<Features>,
    /// Per-tier `skill_overrides` map preserved without merging. Read
    /// by the SkillTool gate and by listing-budget filters so the
    /// model only sees what the resolved override state permits.
    /// Default-empty maps short-circuit every gate to `On` — that is
    /// the no-config behavior PR2 ships.
    pub skill_overrides: Arc<coco_config::SkillOverrideTiers>,
    /// schema validation of the tool-filter pipeline — extra tools the active
    /// model adds beyond the baseline + baseline tools it excludes.
    /// Resolved once at session start (or on `/model` switch).
    pub tool_overrides: Arc<ToolOverrides>,
    /// Layer 4 of the tool-filter pipeline — agent allow/deny.
    /// Constructed from `AgentDefinition.allowed_tools` /
    /// `disallowed_tools` when a subagent spawns; `unrestricted()` for
    /// the top-level session.
    pub tool_filter: ToolFilter,

    /// Wire-names of deferred tools the model has discovered via
    /// `ToolSearch`. Snapshot of [`coco_types::ToolAppState::discovered_tool_names`]
    /// for the current turn — `Arc<HashSet>` so the filter pipeline
    /// can consult it without locking. A deferred tool whose name is
    /// in this set is treated as if `should_defer() == false` by
    /// [`crate::ToolRegistry::loaded_tools`], so its full schema is
    /// sent on the next request.
    ///
    /// Empty default = pre-discovery state; deferred tools stay hidden
    /// until the model finds them via `ToolSearch`.
    pub discovered_tool_names: Arc<HashSet<String>>,

    /// Whether the current model supports Anthropic's server-side
    /// `tool_reference` expansion (`tool-search-tool-2025-10-19`).
    /// Populated by `ToolContextFactory::build` from the active runtime
    /// snapshot's `ModelInfo`.
    ///
    /// When `true`, `ToolSearchTool::execute` emits matches as
    /// `tool_reference` content blocks (via
    /// `ToolResultContentPart::Custom`) so the Anthropic server
    /// expands their schemas inline — keeping the client-side `tools`
    /// array constant across turns (cache-friendly). The
    /// `discovered_tool_names` patch is **skipped** on this path
    /// because the discovery state lives in message history, not in
    /// `ToolAppState`.
    ///
    /// When `false`, the runtime falls back to the
    /// [`Self::model_supports_client_side_tool_search`] path (if also
    /// declared) — text envelope + `AppStatePatch` adding matches to
    /// `discovered_tool_names` so the next turn's `tools` array
    /// surfaces the schemas client-side (one cache break per
    /// discovery).
    pub model_supports_tool_reference: bool,

    /// Whether the current model has been validated against coco-rs's
    /// client-side `ToolSearch` promotion path. Mirrors
    /// [`coco_types::Capability::ClientSideToolSearch`] from the
    /// resolved `ModelInfo`.
    ///
    /// Combined with [`Self::model_supports_tool_reference`] +
    /// [`coco_types::Feature::ToolSearch`] to form the runtime
    /// activation predicate (see [`Self::tool_search_active`]).
    ///
    /// Default `false` for unknown / user-declared models so the
    /// runtime falls back to the safe "eager-load every tool"
    /// behavior — the user can opt a custom model in by adding the
    /// capability via `~/.coco/models.json`.
    pub model_supports_client_side_tool_search: bool,

    /// Whether this turn has anything ToolSearch can usefully surface:
    /// at least one filtered, undiscovered deferred tool or one MCP server
    /// still pending bootstrap. Computed by app/query for real turns.
    pub tool_search_has_candidates: bool,

    // ── Core State ──
    /// Structured abort signal for tool execution.
    pub abort: ToolAbortSignal,
    /// Post-budget message snapshot the engine just sent to the model
    /// this turn. Shared via outer `Arc` so every tool in the batch
    /// observes byte-identical history; inner `Arc<Message>` lets
    /// individual messages be shared with `MessageHistory` without
    /// deep clones. Immutable for the lifetime of the ctx — tools never
    /// mutate it — it is read-only for the lifetime of the ctx.
    ///
    /// Set after `applyToolResultBudget` / `microcompact` /
    /// `applyCollapses` / `autocompact`. Empty `Vec` when no history
    /// has been built yet (test stubs, pre-first-turn).
    pub messages: Arc<Vec<Arc<Message>>>,
    /// Permission context (mode + rules).
    pub permission_context: ToolPermissionContext,

    // ── Agent Identity ──
    /// Current tool use ID (set per tool call).
    pub tool_use_id: Option<String>,
    /// UUID of the user message that triggered this turn.
    /// Used by file history to key snapshots to user messages (not tool calls).
    pub user_message_id: Option<String>,
    /// Agent running this tool.
    pub agent_id: Option<AgentId>,
    /// Agent type.
    pub agent_type: Option<AgentTypeId>,
    /// Snapshot of the active agent definition catalog. AgentTool reads
    /// it to resolve `subagent_type` → `Arc<AgentDefinition>` before
    /// building an `AgentSpawnRequest`, so the spawn-time resolver can
    /// consult the definition's `model` and `model_role` fields. Built
    /// once at session bootstrap (or after `/agents reload`); cheap to
    /// clone — `Arc`-shared.
    ///
    /// `None` means the catalog isn't installed (legacy/test path);
    /// AgentTool degrades to subagent_type→role mapping alone.
    pub agent_catalog: Option<Arc<coco_subagent::AgentCatalogSnapshot>>,

    /// Snapshot of the parent session's resolved provider+API+model
    /// identity. Captured at engine bootstrap from the runtime registry
    /// snapshot and threaded onto every `ToolUseContext`.
    ///
    /// `AgentTool::execute` reads this to construct
    /// `SpawnMode::Fork { parent_snapshot, .. }` — the snapshot lives
    /// **inside** the Fork variant non-optionally, so the type system
    /// forbids constructing Fork without a snapshot. When this field is
    /// `None`, `AgentTool` refuses to enter Fork mode (returns
    /// `ExecutionFailed`) rather than fall back to a live-runtime model
    /// resolution that would silently break cache parity.
    ///
    /// `None` is the legacy/test path — production engines populate it
    /// at bootstrap; tests pass `None` and never trigger Fork mode.
    pub parent_runtime_snapshot: Option<Arc<coco_types::SubagentRuntimeSnapshot>>,

    // ── File Tracking ──
    /// File reading limits.
    pub file_reading_limits: FileReadingLimits,
    /// Glob search limits.
    pub glob_limits: GlobLimits,

    // ── Tracking Sets (session-scoped dedup) ──
    /// Paths that triggered nested memory loading.
    ///
    /// Every successful Read pushes the path here so
    /// `getNestedMemoryAttachments` can load any nested CLAUDE.md /
    /// memory files in the file's ancestry on the next turn boundary.
    ///
    /// Wrapped in `Arc<RwLock<>>` so concurrent-safe tools sharing a
    /// cloned context all push into the same set, mirroring the
    /// `dynamic_skill_dir_triggers` design.
    pub nested_memory_attachment_triggers: Arc<RwLock<HashSet<String>>>,
    /// Already-loaded nested memory file paths.
    pub loaded_nested_memory_paths: HashSet<String>,
    /// Directories that triggered dynamic skill discovery.
    ///
    /// When Read/Write/Edit touch a file, we walk up to find any
    /// `.coco/skills/` ancestor dir and record it here. The app/query
    /// layer drains this set after the tool batch completes and asks the
    /// SkillManager to load any newly-discovered dirs.
    ///
    /// Wrapped in `Arc<RwLock<>>` so concurrent-safe tools sharing a
    /// cloned context all push into the same set, and so the app/query
    /// drain sees everything from the just-completed batch.
    pub dynamic_skill_dir_triggers: Arc<RwLock<HashSet<String>>>,
    /// Files that triggered a conditional-skill activation check.
    ///
    /// `activateConditionalSkillsForPaths(filePaths, cwd)` runs against
    /// every file touched by Read/Write/Edit/Bash. Path-gated
    /// skills (`paths` frontmatter) whose patterns match get promoted
    /// into the visible pool. We collect the raw file paths here and
    /// let the app/query drain dispatch them to
    /// `SkillsSource::activate_skills_for_paths` at turn boundary.
    ///
    /// Sibling of [`Self::dynamic_skill_dir_triggers`]: same shared
    /// `Arc<RwLock<>>` rationale (concurrent siblings push into one
    /// set; one drain per batch).
    pub dynamic_skill_path_triggers: Arc<RwLock<HashSet<String>>>,
    /// Skill names discovered during this session.
    pub discovered_skill_names: HashSet<String>,

    // ── Decision Tracking ──
    /// Per-tool execution decisions (accept/reject).
    pub tool_decisions: HashMap<String, ToolDecision>,

    // ── Flags ──
    /// Whether this context is running as a teammate in a swarm team.
    ///
    /// NOT the same as `agent_id.is_some()`. Regular subagents (Agent tool
    /// spawns) have `agent_id` set but are NOT
    /// teammates. Teammates are swarm members that coordinate via mailbox.
    /// Set by the team spawner; tools check this for teammate-specific behavior
    /// (e.g., ExitPlanMode bypasses permission UI for teammates).
    pub is_teammate: bool,

    /// Whether this context is specifically an in-process teammate.
    /// Pane teammates are separate processes and may run background
    /// subagents; in-process teammates may not.
    pub is_in_process_teammate: bool,

    /// When `true`, this teammate MUST request plan approval from the
    /// team lead before exiting plan mode. Tied to the role definition
    /// in the team file or the
    /// `COCO_PLAN_MODE_REQUIRED` env var. When `false`,
    /// teammates in plan mode can exit "voluntarily" without leader
    /// approval (the tool skips the mailbox write and restores mode
    /// locally like a non-swarm session).
    ///
    /// Only meaningful when `is_teammate == true`. Non-teammate
    /// contexts ignore this field.
    pub plan_mode_required: bool,

    /// This teammate's own agent name (swarm identity). Pre-resolved by
    /// the engine from its configured identity + env fallback so tools
    /// don't each re-read process env.
    /// `None` in non-swarm sessions.
    pub agent_name: Option<String>,

    /// The team name this teammate belongs to. Same rationale as
    /// [`Self::agent_name`].
    pub team_name: Option<String>,

    /// When `true`, ExitPlanMode records TS-shaped pending verification
    /// state so the main-agent reminder can request a later
    /// `VerifyPlanExecution` call.
    /// Deprecated legacy verify-plan flow. Enabled only via explicit
    /// `settings.plan_mode.verify_execution`.
    pub plan_verify_execution: bool,
    /// Plan-mode interview-phase flag — drives the `EnterPlanMode`
    /// post-execute instruction text variant. Source is
    /// `settings.plan_mode.workflow == Interview` only (no Growthbook,
    /// no env var). Mirrors the same field on
    /// `coco_tool_runtime::PromptOptions` and the
    /// `is_plan_interview_phase` field on
    /// `coco_system_reminder::GeneratorContext`.
    pub is_plan_interview_phase: bool,
    /// Whether user modified input during permission prompt.
    pub user_modified: bool,
    /// Require can_use_tool check before execution.
    pub require_can_use_tool: bool,
    /// Preserve tool use results (don't tombstone during compaction).
    pub preserve_tool_use_results: bool,

    // ── Cached Prompt ──
    /// Rendered system prompt (for tools that need prompt context).
    pub rendered_system_prompt: Option<String>,
    /// Experimental critical system reminder.
    pub critical_system_reminder: Option<String>,

    // ── IDs for active tool tracking ──
    /// Currently in-progress tool use IDs.
    pub in_progress_tool_use_ids: Arc<RwLock<HashSet<String>>>,

    // ── LLM Side Queries ──
    /// Handle for making LLM side-queries from tools.
    pub side_query: SideQueryHandle,

    // ── MCP ──
    /// Handle for MCP operations (list/read resources, call tools, auth).
    pub mcp: McpHandleRef,

    // ── LSP ──
    /// Handle for LSP code-intelligence operations. `NoOpLspHandle` in
    /// sessions without a configured language server — its
    /// `is_connected()` returns `false`, which combined with
    /// `LspTool::is_enabled` hides the tool from the model's tool list
    /// entirely.
    pub lsp: LspHandleRef,

    // ── Scheduling ──
    /// Handle for cron/trigger operations.
    pub schedules: ScheduleStoreRef,

    // ── Agent Operations ──
    /// Handle for agent spawning, messaging, and team management.
    pub agent: AgentHandleRef,

    // ── Skill Operations ──
    /// Handle for skill invocation (inline expansion or forked
    /// agent). Installed by the query-layer factory; defaults to
    /// `NoOpSkillHandle` (returns `Unavailable`) so tests and
    /// subagent sessions without a configured skill runtime fail
    /// cleanly with a model-visible error rather than a panic.
    ///
    /// Phase 7 of the agent-loop refactor moved this off
    /// `AgentHandle::resolve_skill` — skill runtime is its own
    /// concern.
    pub skill: crate::skill_handle::SkillHandleRef,

    // ── Swarm Mailbox ──
    /// Handle for writing protocol messages to swarm mailboxes.
    /// Used by ExitPlanModeTool (teammate plan_approval_request) and
    /// SendMessageTool (generic team-to-team messages). `NoOpMailboxHandle`
    /// in non-swarm contexts so tool calls fail fast with a clear error
    /// rather than silently dropping the message.
    pub mailbox: crate::MailboxHandleRef,

    // ── Pending-Message Queue ──
    /// In-memory FIFO of pending messages per recipient agent. When a running agent
    /// receives a `SendMessage` from a peer, the message is queued here
    /// and surfaced via the `agent_pending_messages` system-reminder on
    /// the recipient's next turn. `NoOpPendingMessageStore` in non-swarm
    /// contexts so tool calls become no-ops.
    pub pending_messages: crate::PendingMessageStoreRef,

    // ── Working Directory Override ──
    /// CWD override for worktree-isolated agents.
    pub cwd_override: Option<PathBuf>,

    // ── Sandboxed-write fence ──
    /// FileWrite / FileEdit / NotebookEdit are restricted to paths under
    /// one of these roots. Empty = no restriction. Threaded in by the
    /// memory crate's forked extraction / auto-dream agents (and any
    /// future caller that needs a memdir-only fence). File-mutation
    /// tools must reject paths outside the fence before touching disk.
    pub allowed_write_roots: Vec<PathBuf>,

    // ── Permission Forwarding ──
    /// Bridge for forwarding permission requests from teammate agents.
    /// None for main agent (uses normal permission pipeline).
    pub permission_bridge: Option<ToolPermissionBridgeRef>,

    // ── Per-Fork Tool Gate ──
    /// Optional per-fork canUseTool callback. When `Some`, app/query's
    /// tool-call preparer runs it before the static permission
    /// evaluator consults `tool.check_permissions`. `Deny`
    /// short-circuits with the message surfaced as the synthesized
    /// `tool_result`. `Allow{updated_input}` rewrites the input passed
    /// to permissions + execute (speculation overlay path-rewrite
    /// hook). `Ask` falls through to the tool's built-in opinion.
    /// `None` preserves pre-canUseTool behavior — the callback is
    /// skipped entirely.
    ///
    /// `require_can_use_tool` (above) controls whether `Pre`-tool-use
    /// hook auto-approve can bypass this callback. When `true`,
    /// callback wins regardless of hook config.
    pub can_use_tool: Option<crate::can_use_tool::CanUseToolHandleRef>,

    // ── Progress Reporting ──
    /// Channel for tool progress updates. Tools send ToolProgress here;
    /// ToolExecutor yields them immediately to the TUI.
    pub progress_tx: Option<ProgressSender>,

    // ── Background Task Management ──
    /// Handle for background task operations (shell tasks, agent tasks).
    pub task_handle: Option<BackgroundTaskHandleRef>,

    // ── Persistent Task List (V2) ──
    /// Shared disk-backed plan-item store used by `TaskCreate`/`TaskGet`/
    /// `TaskList`/`TaskUpdate`/`TaskStop` (when operating on todo tasks)
    /// and `TaskOutput` (todo tasks). `NoOpTaskListHandle` in test
    /// contexts or sessions lacking a resolved config-home path.
    pub task_list: TaskListHandleRef,
    /// Router that can switch the active task list when a leader creates
    /// or deletes an agent team.
    pub team_task_list_router: Option<TeamTaskListRouterRef>,

    // ── Per-Agent TodoWrite (V1) ──
    /// In-memory per-agent checklist store used by `TodoWriteTool`.
    /// Keyed by `agent_id.unwrap_or(session_id)`. Lives for the
    /// process lifetime — never persisted to disk.
    pub todo_list: TodoListHandleRef,

    // ── Hook Pipeline ──
    /// Optional callback into the hook pipeline (PreToolUse / PostToolUse /
    /// PostToolUseFailure). When `None`, the executor skips hook invocations
    /// entirely. The higher-layer orchestrator (`app/query`) implements this
    /// trait by bridging to `coco_hooks::HookRegistry` + `execute_pre_tool_use()`
    /// / `execute_post_tool_use()`.
    pub hook_handle: Option<HookHandleRef>,

    // ── File State ──
    /// Session-level file read state for @mention dedup, edit safety, changed-file detection.
    pub file_read_state: Option<Arc<RwLock<FileReadState>>>,

    // ── File History ──
    /// File history for checkpoint/rewind. Shared across concurrent tool calls.
    pub file_history: Option<Arc<RwLock<FileHistoryState>>>,
    /// Config home directory for file history backup storage.
    pub config_home: Option<PathBuf>,
    /// Session ID for file history backup naming.
    pub session_id_for_history: Option<String>,
    /// Session artifact root used by tool-result persistence helpers.
    /// Storage helpers append `tool-results/` below this directory.
    pub tool_result_session_dir: Option<PathBuf>,
    /// Path to the session transcript, used for post-clear implementation
    /// hints after plan approval.
    pub transcript_path: Option<PathBuf>,
    /// Optional user feedback attached to the approval for this specific tool
    /// call.
    pub approval_feedback: Option<String>,
    /// Trusted tool-specific metadata attached to the approval for this
    /// specific tool call.
    pub permission_resolution_detail: Option<coco_types::PermissionResolutionDetail>,

    // ── Plan mode ──
    /// Resolved plans directory for plan-mode file I/O. Pre-computed by
    /// the engine from `config_home` + project root + `plansDirectory`
    /// setting, so tools can locate the plan file without re-deriving.
    pub plans_dir: Option<PathBuf>,

    // ── App State ──
    /// Read-only handle to the shared cross-turn application state
    /// (plan-mode latches, reminder throttle counters, pending
    /// teammate approvals, live permission mode). Typed struct — see
    /// [`coco_types::ToolAppState`] for the field catalog.
    ///
    /// **Write access is deliberately not exposed** — tools cannot
    /// call `.write()` on this handle. Mutations route through
    /// [`coco_messages::ToolResult::app_state_patch`], applied
    /// post-execute by the executor. Tools return an `AppStatePatch`
    /// modifier; the orchestrator applies them after the concurrent
    /// batch finishes. Rust encodes the same discipline in the type
    /// system so a tool that tries to mutate shared state simply
    /// won't compile.
    pub app_state: Option<coco_types::AppStateReadHandle>,

    // ── Denial Tracking ──
    /// Per-context auto-mode denial tracker.
    ///
    /// `Some(arc)` when this context is a **fork** — the fork holds an
    /// isolated tracker so its denial streak cannot poison the parent
    /// session's circuit breaker — always a fresh tracker for forks.
    /// `None` on the main session context; callers fall back to the
    /// engine-level session tracker.
    ///
    /// Read order at the classifier site:
    /// `ctx.local_denial_tracking` → engine-level session tracker.
    pub local_denial_tracking: Option<Arc<Mutex<DenialTracker>>>,

    // ── Query Tracking ──
    /// Query chain ID for telemetry grouping.
    pub query_chain_id: Option<String>,
    /// Query depth (0 = main, 1+ = subagent).
    pub query_depth: i32,
}

/// File reading limits for tools.
#[derive(Debug, Clone, Default)]
pub struct FileReadingLimits {
    /// Maximum tokens for file content.
    pub max_tokens: Option<i64>,
    /// Maximum file size in bytes.
    pub max_size_bytes: Option<i64>,
}

/// Glob search limits.
#[derive(Debug, Clone, Default)]
pub struct GlobLimits {
    /// Maximum number of glob results.
    pub max_results: Option<i32>,
}

/// A tool execution decision record.
#[derive(Debug, Clone)]
pub struct ToolDecision {
    pub source: String,
    pub decision: ToolDecisionKind,
    pub timestamp: i64,
}

/// Accept or reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDecisionKind {
    Accept,
    Reject,
}

use coco_types::PermissionMode;

impl ToolUseContext {
    /// Legacy token adapter for subprocess/provider APIs that still accept a
    /// plain [`CancellationToken`]. Semantic reason remains on [`Self::abort`].
    pub fn cancel_token(&self) -> CancellationToken {
        self.abort.token()
    }

    /// Clone the context for use in concurrent tool execution.
    ///
    /// Shares Arc-wrapped state (messages, in_progress IDs, app_state, denial tracking)
    /// while cloning value types. Concurrent tools are read-only and self-contained,
    /// so they only need the shared references and config.
    pub fn clone_for_concurrent(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            main_loop_model: self.main_loop_model.clone(),
            thinking_level: self.thinking_level.clone(),
            is_non_interactive: self.is_non_interactive,
            avoid_permission_prompts: self.avoid_permission_prompts,
            max_budget_usd: self.max_budget_usd,
            custom_system_prompt: self.custom_system_prompt.clone(),
            append_system_prompt: self.append_system_prompt.clone(),
            debug: self.debug,
            verbose: self.verbose,
            tool_config: self.tool_config.clone(),
            sandbox_config: self.sandbox_config.clone(),
            sandbox_state: self.sandbox_state.clone(),
            memory_config: self.memory_config.clone(),
            shell_config: self.shell_config.clone(),
            active_shell_tool: self.active_shell_tool,
            shell_provider: self.shell_provider.clone(),
            original_cwd: self.original_cwd.clone(),
            session_cwd: self.session_cwd.clone(),
            web_fetch_config: self.web_fetch_config.clone(),
            web_search_config: self.web_search_config.clone(),
            plan_mode_settings: self.plan_mode_settings.clone(),
            lsp_config: self.lsp_config.clone(),
            features: self.features.clone(),
            skill_overrides: self.skill_overrides.clone(),
            tool_overrides: self.tool_overrides.clone(),
            tool_filter: self.tool_filter.clone(),
            discovered_tool_names: self.discovered_tool_names.clone(),
            model_supports_tool_reference: self.model_supports_tool_reference,
            model_supports_client_side_tool_search: self.model_supports_client_side_tool_search,
            tool_search_has_candidates: self.tool_search_has_candidates,
            abort: self.abort.clone(),
            messages: self.messages.clone(),
            permission_context: self.permission_context.clone(),
            tool_use_id: None, // each concurrent tool gets its own ID
            user_message_id: self.user_message_id.clone(),
            agent_id: self.agent_id.clone(),
            agent_type: self.agent_type.clone(),
            agent_catalog: self.agent_catalog.clone(),
            parent_runtime_snapshot: self.parent_runtime_snapshot.clone(),
            file_reading_limits: self.file_reading_limits.clone(),
            glob_limits: self.glob_limits.clone(),
            // Share both trigger sets across concurrent siblings so all
            // pushes from the batch land in one place for app/query to
            // drain. See field docs on the struct.
            nested_memory_attachment_triggers: self.nested_memory_attachment_triggers.clone(),
            loaded_nested_memory_paths: HashSet::new(),
            dynamic_skill_dir_triggers: self.dynamic_skill_dir_triggers.clone(),
            dynamic_skill_path_triggers: self.dynamic_skill_path_triggers.clone(),
            discovered_skill_names: HashSet::new(),
            tool_decisions: HashMap::new(),
            is_teammate: self.is_teammate,
            is_in_process_teammate: self.is_in_process_teammate,
            plan_mode_required: self.plan_mode_required,
            agent_name: self.agent_name.clone(),
            team_name: self.team_name.clone(),
            plan_verify_execution: self.plan_verify_execution,
            is_plan_interview_phase: self.is_plan_interview_phase,
            user_modified: false,
            require_can_use_tool: self.require_can_use_tool,
            preserve_tool_use_results: self.preserve_tool_use_results,
            rendered_system_prompt: self.rendered_system_prompt.clone(),
            critical_system_reminder: self.critical_system_reminder.clone(),
            in_progress_tool_use_ids: self.in_progress_tool_use_ids.clone(),
            side_query: self.side_query.clone(),
            mcp: self.mcp.clone(),
            lsp: self.lsp.clone(),
            schedules: self.schedules.clone(),
            agent: self.agent.clone(),
            skill: self.skill.clone(),
            mailbox: self.mailbox.clone(),
            pending_messages: self.pending_messages.clone(),
            cwd_override: self.cwd_override.clone(),
            allowed_write_roots: self.allowed_write_roots.clone(),
            permission_bridge: self.permission_bridge.clone(),
            can_use_tool: self.can_use_tool.clone(),
            progress_tx: self.progress_tx.clone(),
            task_handle: self.task_handle.clone(),
            task_list: self.task_list.clone(),
            team_task_list_router: self.team_task_list_router.clone(),
            todo_list: self.todo_list.clone(),
            hook_handle: self.hook_handle.clone(),
            file_read_state: self.file_read_state.clone(),
            file_history: self.file_history.clone(),
            config_home: self.config_home.clone(),
            session_id_for_history: self.session_id_for_history.clone(),
            tool_result_session_dir: self.tool_result_session_dir.clone(),
            transcript_path: self.transcript_path.clone(),
            approval_feedback: self.approval_feedback.clone(),
            permission_resolution_detail: self.permission_resolution_detail.clone(),
            plans_dir: self.plans_dir.clone(),
            app_state: self.app_state.clone(),
            local_denial_tracking: self.local_denial_tracking.clone(),
            query_chain_id: self.query_chain_id.clone(),
            query_depth: self.query_depth,
        }
    }

    /// Clone this context for one concrete tool call while preserving
    /// per-batch state. Unlike [`Self::clone_for_concurrent`], this is
    /// suitable for serial tools too; it only installs the call id.
    pub fn clone_for_tool_call(&self, tool_use_id: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.tool_use_id = Some(tool_use_id.into());
        cloned
    }

    /// Build a context suitable **only** for the registry filter pipeline
    /// — system-reminder tool listings, SessionStarted bootstrap events,
    /// and similar display-only sites that don't run the tool. All
    /// non-filter fields use cheap stub values; do not pass this to
    /// `Tool::execute()`.
    pub fn stub_for_filtering(
        features: Arc<Features>,
        tool_overrides: Arc<ToolOverrides>,
        tool_filter: ToolFilter,
        permission_mode: PermissionMode,
    ) -> Self {
        let mut ctx = Self::test_default_inner();
        ctx.features = features;
        ctx.tool_overrides = tool_overrides;
        ctx.tool_filter = tool_filter;
        ctx.permission_context.mode = permission_mode;
        ctx
    }

    /// Builder: install the `ToolSearch`-discovered tool-name snapshot
    /// onto an existing context. Callers thread this from
    /// `ToolAppState::discovered_tool_names` so the registry filter
    /// pipeline can upgrade discovered deferred tools into the loaded
    /// pool.
    pub fn with_discovered_tool_names(mut self, names: Arc<HashSet<String>>) -> Self {
        self.discovered_tool_names = names;
        self
    }

    /// Effective shell cwd for analysis and spawn.
    ///
    /// Keep this in sync with shell execution semantics: worktree override
    /// first, then the live shared session cwd, then the process cwd.
    pub async fn effective_shell_cwd(&self) -> std::path::PathBuf {
        if let Some(over) = self.cwd_override.clone() {
            over
        } else if let Some(session_cwd) = &self.session_cwd {
            session_cwd.read().await.clone()
        } else {
            std::env::current_dir()
                .ok()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        }
    }

    /// All `/<word>` tokens the user typed in the current turn —
    /// indexed for O(1) gate lookup against canonical skill names
    /// AND aliases. Lines like `/fix-issue 42` contribute
    /// `"fix-issue"`; mid-line slashes are NOT counted (line-anchored).
    ///
    /// Used by
    /// the Skill tool gate to bypass the
    /// `disable_model_invocation` and `skill_overrides ==
    /// user-invocable-only` blocks when the user explicitly
    /// invoked the skill via slash.
    pub fn typed_slashes_in_turn(&self) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let Some(uid) = self.user_message_id.as_deref() else {
            return out;
        };
        let Ok(uid_uuid) = uuid::Uuid::parse_str(uid) else {
            return out;
        };
        for (idx, arc) in self.messages.iter().enumerate().rev() {
            let Message::User(user) = arc.as_ref() else {
                continue;
            };
            if user.uuid != uid_uuid {
                continue;
            }
            extract_slash_tokens(&user.message, &mut out);
            if let Some(prev) = idx.checked_sub(1).and_then(|i| self.messages.get(i))
                && let Message::Attachment(attachment) = prev.as_ref()
                && attachment.kind == coco_types::AttachmentKind::SlashCommandMetadata
                && let Some(message) = attachment.as_api_message()
            {
                extract_command_name_tokens(message, &mut out);
            }
            break;
        }
        out
    }

    /// Builder: install the current model's `ToolSearch`-related
    /// capability flags on a stub context. Used by `engine_prompt`
    /// and `engine_turn_reminders` so the registry filter and the
    /// `deferred_tools_delta` partitioner see the same activation
    /// predicate the runtime would see.
    pub fn with_model_capabilities(
        mut self,
        supports_tool_reference: bool,
        supports_client_side_tool_search: bool,
    ) -> Self {
        self.model_supports_tool_reference = supports_tool_reference;
        self.model_supports_client_side_tool_search = supports_client_side_tool_search;
        self
    }

    /// Builder: install the selected model-facing shell tool on a filtering
    /// stub. Real execution contexts get this from `QueryEngineConfig`.
    pub fn with_active_shell_tool(mut self, active_shell_tool: ActiveShellTool) -> Self {
        self.active_shell_tool = active_shell_tool;
        self
    }

    /// Builder: install the current turn's ToolSearch candidate gate.
    pub fn with_tool_search_candidates(mut self, has_candidates: bool) -> Self {
        self.tool_search_has_candidates = has_candidates;
        self
    }

    /// Capability/feature predicate before checking whether any candidates
    /// are currently searchable.
    pub fn tool_search_supported(&self) -> bool {
        self.features.enabled(coco_types::Feature::ToolSearch)
            && (self.model_supports_tool_reference || self.model_supports_client_side_tool_search)
    }

    /// Effective `ToolSearch` activation for the current turn.
    ///
    /// Three-way predicate combining:
    /// 1. User-facing [`coco_types::Feature::ToolSearch`] gate.
    /// 2. Model capability — at least one of
    ///    [`Self::model_supports_tool_reference`] (server-side, cache-friendly)
    ///    or [`Self::model_supports_client_side_tool_search`]
    ///    (universal, costs cache breaks on Anthropic) must be declared.
    /// 3. A current candidate source — at least one undiscovered deferred
    ///    tool or one pending MCP server.
    ///
    /// When `false`:
    ///   - [`crate::ToolRegistry::loaded_tools`] short-circuits the
    ///     deferral filter — every enabled tool's schema lands on
    ///     turn 1 (standard mode equivalent).
    ///   - [`crate::ToolRegistry::deferred_tools`] returns empty.
    ///   - `ToolSearchTool::is_enabled` returns `false`; the tool
    ///     is hidden from the model.
    ///
    /// This is the canonical site for the predicate so registry /
    /// tool / engine_prompt agree byte-for-byte.
    pub fn tool_search_active(&self) -> bool {
        self.tool_search_supported() && self.tool_search_has_candidates
    }

    /// Create a minimal context for testing.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_default() -> Self {
        Self::test_default_inner()
    }

    fn test_default_inner() -> Self {
        Self {
            tools: Arc::new(ToolRegistry::new()),
            main_loop_model: "test-model".into(),
            thinking_level: None,
            is_non_interactive: false,
            avoid_permission_prompts: false,
            max_budget_usd: None,
            custom_system_prompt: None,
            append_system_prompt: None,
            debug: false,
            verbose: false,
            tool_config: coco_config::ToolConfig::default(),
            sandbox_config: coco_config::SandboxSettings::default(),
            sandbox_state: None,
            memory_config: coco_config::MemoryConfig::default(),
            shell_config: coco_config::ShellConfig::default(),
            active_shell_tool: ActiveShellTool::default(),
            shell_provider: None,
            original_cwd: None,
            session_cwd: None,
            web_fetch_config: coco_config::WebFetchConfig::default(),
            web_search_config: coco_config::WebSearchConfig::default(),
            plan_mode_settings: coco_config::PlanModeSettings::default(),
            lsp_config: coco_config::LspConfig::default(),
            features: Arc::new(Features::with_defaults()),
            skill_overrides: Arc::new(coco_config::SkillOverrideTiers::default()),
            tool_overrides: Arc::new(ToolOverrides::none()),
            tool_filter: ToolFilter::unrestricted(),
            discovered_tool_names: Arc::new(HashSet::new()),
            model_supports_tool_reference: false,
            model_supports_client_side_tool_search: false,
            tool_search_has_candidates: false,
            abort: ToolAbortSignal::from_turn(TurnAbortController::new().signal()),
            messages: Arc::new(Vec::new()),
            permission_context: ToolPermissionContext {
                mode: PermissionMode::BypassPermissions,
                additional_dirs: HashMap::new(),
                allow_rules: HashMap::new(),
                deny_rules: HashMap::new(),
                ask_rules: HashMap::new(),
                bypass_available: true,
                pre_plan_mode: None,
                stripped_dangerous_rules: None,
                session_plan_file: None,
                permission_rule_source_roots: HashMap::new(),
            },
            tool_use_id: None,
            user_message_id: None,
            agent_id: None,
            agent_type: None,
            agent_catalog: None,
            parent_runtime_snapshot: None,
            file_reading_limits: FileReadingLimits::default(),
            glob_limits: GlobLimits::default(),
            nested_memory_attachment_triggers: Arc::new(RwLock::new(HashSet::new())),
            loaded_nested_memory_paths: HashSet::new(),
            dynamic_skill_dir_triggers: Arc::new(RwLock::new(HashSet::new())),
            dynamic_skill_path_triggers: Arc::new(RwLock::new(HashSet::new())),
            discovered_skill_names: HashSet::new(),
            tool_decisions: HashMap::new(),
            is_teammate: false,
            is_in_process_teammate: false,
            plan_mode_required: false,
            agent_name: None,
            team_name: None,
            plan_verify_execution: false,
            is_plan_interview_phase: false,
            user_modified: false,
            require_can_use_tool: false,
            preserve_tool_use_results: false,
            rendered_system_prompt: None,
            critical_system_reminder: None,
            in_progress_tool_use_ids: Arc::new(RwLock::new(HashSet::new())),
            side_query: Arc::new(crate::side_query::NoOpSideQuery),
            mcp: Arc::new(crate::mcp_handle::NoOpMcpHandle),
            lsp: Arc::new(crate::lsp_handle::NoOpLspHandle),
            schedules: Arc::new(crate::schedule_store::NoOpScheduleStore),
            agent: Arc::new(crate::agent_handle::NoOpAgentHandle),
            skill: Arc::new(crate::skill_handle::NoOpSkillHandle),
            mailbox: Arc::new(crate::NoOpMailboxHandle),
            pending_messages: Arc::new(crate::NoOpPendingMessageStore),
            cwd_override: None,
            allowed_write_roots: Vec::new(),
            permission_bridge: None,
            can_use_tool: None,
            progress_tx: None,
            task_handle: None,
            task_list: Arc::new(crate::task_list_handle::InMemoryTaskListHandle::new()),
            team_task_list_router: None,
            todo_list: Arc::new(crate::task_list_handle::InMemoryTodoListHandle::new()),
            hook_handle: None,
            file_read_state: None,
            file_history: None,
            config_home: None,
            session_id_for_history: None,
            tool_result_session_dir: None,
            transcript_path: None,
            approval_feedback: None,
            permission_resolution_detail: None,
            plans_dir: None,
            app_state: None,
            local_denial_tracking: None,
            query_chain_id: None,
            query_depth: 0,
        }
    }
}

/// Extract every `/<word>` token that begins a line of any text
/// content part of `msg`, normalised to the bare name (no leading
/// `/`). Mid-line slashes are skipped (line-anchored).
///
/// `<word>` extends until the first whitespace; the token can
/// contain any non-whitespace character so kebab-case (`/fix-issue`),
/// colon-separated MCP names (`/server:resource`), and digits all
/// surface unchanged.
fn extract_slash_tokens(
    msg: &coco_messages::LlmMessage,
    out: &mut std::collections::HashSet<String>,
) {
    let coco_messages::LlmMessage::User { content, .. } = msg else {
        return;
    };
    for part in content {
        let coco_messages::UserContent::Text(text) = part else {
            continue;
        };
        for line in text.text.lines() {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix('/') else {
                continue;
            };
            let token: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if !token.is_empty() {
                out.insert(token);
            }
        }
    }
}

fn extract_command_name_tokens(
    msg: &coco_messages::LlmMessage,
    out: &mut std::collections::HashSet<String>,
) {
    let coco_messages::LlmMessage::User { content, .. } = msg else {
        return;
    };
    for part in content {
        let coco_messages::UserContent::Text(text) = part else {
            continue;
        };
        extract_command_name_tags(&text.text, out);
    }
}

fn extract_command_name_tags(text: &str, out: &mut std::collections::HashSet<String>) {
    let mut rest = text;
    let open = "<command-name>";
    let close = "</command-name>";
    while let Some(start) = rest.find(open) {
        let value_start = start + open.len();
        let Some(end_offset) = rest[value_start..].find(close) else {
            break;
        };
        let value = rest[value_start..value_start + end_offset].trim();
        let value = value.strip_prefix('/').unwrap_or(value);
        let token: String = value.chars().take_while(|c| !c.is_whitespace()).collect();
        if !token.is_empty() {
            out.insert(token);
        }
        rest = &rest[value_start + end_offset + close.len()..];
    }
}

#[cfg(test)]
mod user_typed_slash_tests {
    use super::*;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    use coco_messages::UserMessage;
    use uuid::Uuid;

    fn make_user(text: &str, uuid: Uuid) -> Arc<Message> {
        Arc::new(Message::User(UserMessage {
            message: LlmMessage::user_text(text),
            uuid,
            timestamp: String::new(),
            is_visible_in_transcript_only: false,
            is_virtual: false,
            is_compact_summary: false,
            permission_mode: None,
            origin: None,
            parent_tool_use_id: None,
        }))
    }

    fn make_attachment(text: &str, uuid: Uuid) -> Arc<Message> {
        let mut attachment = coco_messages::AttachmentMessage::api(
            coco_types::AttachmentKind::CriticalSystemReminder,
            LlmMessage::user_text(text),
        );
        attachment.uuid = uuid;
        Arc::new(Message::Attachment(attachment))
    }

    fn make_slash_metadata(text: &str) -> Arc<Message> {
        Arc::new(Message::Attachment(coco_messages::AttachmentMessage::api(
            coco_types::AttachmentKind::SlashCommandMetadata,
            LlmMessage::user_text(text),
        )))
    }

    fn ctx_with(messages: Vec<Arc<Message>>, user_msg_id: Option<String>) -> ToolUseContext {
        let mut ctx = ToolUseContext::test_default_inner();
        ctx.messages = Arc::new(messages);
        ctx.user_message_id = user_msg_id;
        ctx
    }

    #[test]
    fn empty_set_when_user_message_id_unset() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(vec![make_user("/foo", uid)], None);
        assert!(ctx.typed_slashes_in_turn().is_empty());
    }

    #[test]
    fn captures_exact_slash_on_a_line() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(vec![make_user("/foo", uid)], Some(uid.to_string()));
        let set = ctx.typed_slashes_in_turn();
        assert!(set.contains("foo"));
    }

    #[test]
    fn captures_slash_then_args() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![make_user("/fix-issue 42 please", uid)],
            Some(uid.to_string()),
        );
        let set = ctx.typed_slashes_in_turn();
        assert!(set.contains("fix-issue"));
        // Args after the first whitespace must NOT be captured as tokens.
        assert!(!set.contains("42"));
        assert!(!set.contains("please"));
    }

    #[test]
    fn captures_slash_on_first_line_of_multi_line_prompt() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![make_user("/deploy\nplease verify after", uid)],
            Some(uid.to_string()),
        );
        assert!(ctx.typed_slashes_in_turn().contains("deploy"));
    }

    #[test]
    fn distinguishes_prefix_from_longer_token() {
        let uid = Uuid::new_v4();
        // `/foobar` becomes token "foobar"; lookup for "foo" must miss.
        let ctx = ctx_with(vec![make_user("/foobar", uid)], Some(uid.to_string()));
        let set = ctx.typed_slashes_in_turn();
        assert!(set.contains("foobar"));
        assert!(!set.contains("foo"));
    }

    #[test]
    fn ignores_mid_line_slashes() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![make_user("please run the /foo command for me", uid)],
            Some(uid.to_string()),
        );
        // Mid-line `/foo` is not a user-initiated invocation (anchored at line start).
        assert!(!ctx.typed_slashes_in_turn().contains("foo"));
    }

    #[test]
    fn skips_non_user_messages_and_mismatched_uuids() {
        let triggering_uid = Uuid::new_v4();
        let other_uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![
                make_user("/foo", other_uid),
                make_user("regular prompt", triggering_uid),
            ],
            Some(triggering_uid.to_string()),
        );
        // The `/foo` line came from a different turn (different uuid) —
        // doesn't count.
        assert!(!ctx.typed_slashes_in_turn().contains("foo"));
    }

    #[test]
    fn captures_multiple_slashes_in_one_turn_for_alias_lookup() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![make_user("/alpha\n/beta arg", uid)],
            Some(uid.to_string()),
        );
        let set = ctx.typed_slashes_in_turn();
        // Both `/alpha` and `/beta` are line-anchored → captured.
        // The alias-aware Skill tool gate checks each candidate name
        // against this set, so a skill with `aliases: [alpha]` can
        // bypass the gate when the user typed `/alpha` even though
        // the model invokes the canonical name.
        assert!(set.contains("alpha"));
        assert!(set.contains("beta"));
    }

    #[test]
    fn captures_slash_command_metadata_tag_for_current_turn() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![
                make_slash_metadata(
                    "<command-message>simplify</command-message>\n\
                     <command-name>/simplify</command-name>\n\
                     <command-args>cleanup</command-args>",
                ),
                make_user("expanded prompt", uid),
            ],
            Some(uid.to_string()),
        );
        let set = ctx.typed_slashes_in_turn();
        assert!(set.contains("simplify"));
        assert!(!set.contains("cleanup"));
    }

    #[test]
    fn ignores_command_name_tag_in_plain_user_text() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![make_user(
                "please inspect <command-name>/debug</command-name>",
                uid,
            )],
            Some(uid.to_string()),
        );
        assert!(!ctx.typed_slashes_in_turn().contains("debug"));
    }

    #[test]
    fn ignores_command_name_tag_in_generic_attachment() {
        let uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![
                make_attachment("<command-name>/debug</command-name>", Uuid::new_v4()),
                make_user("expanded prompt", uid),
            ],
            Some(uid.to_string()),
        );
        assert!(!ctx.typed_slashes_in_turn().contains("debug"));
    }

    #[test]
    fn ignores_stale_slash_metadata_from_prior_turn() {
        let old_uid = Uuid::new_v4();
        let current_uid = Uuid::new_v4();
        let ctx = ctx_with(
            vec![
                make_slash_metadata("<command-name>/simplify</command-name>"),
                make_user("old expanded prompt", old_uid),
                make_user("plain current prompt", current_uid),
            ],
            Some(current_uid.to_string()),
        );
        assert!(!ctx.typed_slashes_in_turn().contains("simplify"));
    }
}
