//! Configuration + result types for `QueryEngine`.
//!
//! Extracted from `engine.rs` so the orchestration module stays focused on the
//! session loop. Pure data types — no behavior lives here.

use coco_config::MemoryConfig;
use coco_config::PlanModeSettings;
use coco_config::SandboxSettings;
use coco_config::ShellConfig;
use coco_config::ToolConfig;
use coco_config::WebFetchConfig;
use coco_config::WebSearchConfig;
use coco_messages::CostTracker;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_types::ActiveShellTool;
use coco_types::Features;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PromptCacheConfig;
use coco_types::ThinkingLevel;
use coco_types::TokenUsage;
use coco_types::ToolFilter;
use coco_types::ToolOverrides;
use std::sync::Arc;

/// Hard cap on how many recovery cycles (post-escalation) we attempt before
/// giving up.
pub(crate) const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: i32 = 3;

/// Default context window when no `ModelInfo` is wired (mocked clients,
/// test fixtures) and the caller didn't pass an explicit value. Chosen
/// to match the headline-model Anthropic / OpenAI bands at the time of
/// import; conservative enough not to over-block, liberal enough to let
/// typical sessions through.
pub(crate) const DEFAULT_CONTEXT_WINDOW: i64 = 200_000;

/// Default cap on consecutive `StructuredOutput` retries. Overridable
/// via [`coco_config::EnvKey::CocoMaxStructuredOutputRetries`].
pub(crate) const DEFAULT_MAX_STRUCTURED_OUTPUT_RETRIES: u32 = 5;

/// Resolved retry cap: env override wins, otherwise
/// [`DEFAULT_MAX_STRUCTURED_OUTPUT_RETRIES`]. Single read site shared
/// by the engine loop (`engine.rs::run_session_loop` terminal cap) and
/// the session adapter (`engine_session.rs` error-message render).
pub(crate) fn max_structured_output_retries() -> u32 {
    coco_config::env::env_opt_u32(coco_config::EnvKey::CocoMaxStructuredOutputRetries)
        .unwrap_or(DEFAULT_MAX_STRUCTURED_OUTPUT_RETRIES)
}

/// Why the loop is continuing instead of exiting.
///
/// Enables tests to verify recovery paths fired without inspecting
/// message contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinueReason {
    /// Normal tool-call loop: model returned tool calls, process and continue.
    NextTurn,
    /// Reactive compaction after prompt-too-long error.
    ReactiveCompactRetry,
    /// Max output tokens escalation — retry once with the active model's
    /// opt-in `ModelInfo.max_output_tokens_escalate` ceiling instead of
    /// the baseline cap. Per-model; no escalation when the field is
    /// unset.
    MaxOutputTokensEscalate,
    /// Max output tokens recovery attempt.
    MaxOutputTokensRecovery { attempt: i32 },
    /// Stop hook requested blocking continuation.
    StopHookBlocking,
    /// Token budget allows one more continuation.
    TokenBudgetContinuation,
    /// Context collapse drain retry.
    CollapseDrainRetry { committed: i32 },
}

/// Per-engine permission derivation — the Rust analog of TS
/// `createSubagentContext` + `agentGetAppState`. Applied in
/// `ToolContextFactory::build` on top of the shared live base (read-through
/// from the parent's `ToolAppState`), producing the engine's
/// `ToolPermissionContext` without ever mutating the shared base.
///
/// `deny`/`ask`/`mode` are inherited from the base read-through (TS parity).
/// This struct only carries the per-engine *deltas*.
#[derive(Debug, Clone, Default)]
pub struct PermissionDerivation {
    /// TS `allowedTools` replace-on-restrict. When `Some`, the derived allow
    /// map keeps ONLY the parent's `CliArg`-source rules plus these (as
    /// `Session`-source), dropping the parent's other allow sources. `None`
    /// inherits the parent's full allow set. Mirrors `runAgent.ts:469-479`.
    pub allowed_tools_replace: Option<Vec<PermissionRule>>,
    /// Optional per-agent mode override (agent-definition `permissionMode`).
    /// Does not override parent `Bypass`/`AcceptEdits`/`Auto`.
    pub mode_override: Option<PermissionMode>,
    /// Read-scope dirs layered on top of the inherited base (parent cwd bridge
    /// for worktree-isolated subagents + explicit inherited dirs).
    pub extra_additional_dirs: std::collections::HashMap<String, coco_types::AdditionalWorkingDir>,
}

/// Configuration for the query engine.
#[derive(Debug, Clone)]
pub struct QueryEngineConfig {
    /// Maximum agentic turns before stopping. `None` = unbounded (the model
    /// loops until it naturally stops). Subagents/forks set an explicit
    /// `Some(_)` cap.
    pub max_turns: Option<i32>,
    /// Session-level total token budget (input + output, accumulated
    /// across every API call in this loop invocation). Drives
    /// [`crate::budget::BudgetTracker`]'s 90% nudge / diminishing-returns
    /// stop logic. `None` = unbounded.
    ///
    /// Per-call `max_output_tokens` is **not** in this struct — it lives
    /// on each model's [`coco_config::ModelInfo::max_output_tokens`] (the
    /// baseline) and [`coco_config::ModelInfo::max_output_tokens_escalate`]
    /// (the opt-in Phase-1 retry ceiling). This separation matches the
    /// `ModelRole` × multi-LLM swap surface — plan-mode / Fast role /
    /// fallback advance each carry their own ModelInfo.
    pub total_token_budget: Option<i64>,
    /// Per-call prompt-cache directive. Main sessions set this when the
    /// active provider/model supports prompt caching; forked sessions
    /// inherit it from the parent cache slot and may set
    /// `skip_cache_write`.
    pub prompt_cache: Option<PromptCacheConfig>,
    /// System prompt to prepend.
    pub system_prompt: Option<String>,
    /// Append to system prompt (after CLAUDE.md).
    pub append_system_prompt: Option<String>,
    /// Model name for tool context.
    pub model_id: String,
    /// Permission mode for tool execution.
    pub permission_mode: PermissionMode,
    /// Whether this session may transition into `BypassPermissions`.
    ///
    /// Static capability set once at session bootstrap from the CLI
    /// (`--dangerously-skip-permissions` OR `--allow-dangerously-skip-permissions`)
    /// and policy killswitch. Threaded into
    /// `ToolPermissionContext.bypass_available` on every tool-context
    /// rebuild so the Plan-mode auto-allow + Shift+Tab cycle gate stay
    /// aligned.
    pub bypass_permissions_available: bool,
    /// Context window size in tokens (for compaction trigger).
    pub context_window: i64,
    /// Max output tokens for the model (used in effective window calculation).
    pub max_output_tokens: i64,
    /// Maximum budget in USD (None = unlimited).
    pub max_budget_usd: Option<f64>,
    /// Per-turn output-token budget surfaced to the model via the
    /// `output_token_usage` reminder (`turn / budget · session`). Mirrors
    /// claude-code's `output_config.task_budget.total` — a programmatic/SDK
    /// param with no CLI flag in TS. `None` (default) leaves the reminder
    /// dormant. Informational only here; API-side enforcement
    /// (Anthropic `task-budgets` beta) is a separate provider concern.
    pub output_token_budget: Option<i64>,
    /// Enable streaming tool execution (tools execute during API streaming).
    pub streaming_tool_execution: bool,
    /// Whether this is a non-interactive (SDK/script) session. Drives
    /// session-level side effects only (self-fork suppression, the "sdk"
    /// label, prompt assembly) — NOT permission decisions.
    pub is_non_interactive: bool,
    /// Whether the session cannot show an interactive permission prompt, so a
    /// residual `Ask` must fail closed (Deny) instead of silently auto-allowing.
    /// True for forked / async subagents and headless `-p`. Kept distinct
    /// from `is_non_interactive`: the top-level `-p` flag is non-interactive
    /// yet still propagates `Ask` to an SDK `canUseTool` consumer, so the
    /// two concepts must be independently settable.
    pub avoid_permission_prompts: bool,
    /// Debug-logging surface for tools (CLI `--debug`). Visible on
    /// `ToolUseContext.debug`. Defaults to `false`.
    pub debug: bool,
    /// Verbose-logging surface for tools (CLI `--verbose`). Visible on
    /// `ToolUseContext.verbose`. Defaults to `false`.
    pub verbose: bool,
    /// Thinking level applied to the main-loop model for this session.
    /// Surfaced on `ToolUseContext.thinking_level` so tools (and tool-
    /// spawned subqueries) see the same reasoning budget the engine is
    /// currently driving the LLM with.
    pub thinking_level: Option<ThinkingLevel>,
    /// Whether the main model call should request fast-mode behavior.
    pub fast_mode: bool,
    /// Session identifier for hook orchestration context.
    pub session_id: String,
    /// Project root directory for hook orchestration context.
    pub project_dir: Option<std::path::PathBuf>,
    /// Optional live permission rules read on every tool-context build.
    /// The leader can send `team_permission_update` while a teammate is
    /// mid-turn, and the next tool permission check sees the new
    /// allow/deny/ask rule without restarting the teammate.
    pub live_permission_rules: Option<Arc<tokio::sync::RwLock<Vec<PermissionRule>>>>,
    /// Optional live permission mode read on every tool-context build.
    /// This is the teammate analog of `ToolAppState.permission_mode`
    /// for sessions that do not own a full app-state handle.
    pub live_permission_mode: Option<Arc<tokio::sync::RwLock<PermissionMode>>>,
    /// Root directories used to resolve leading-`/` path permission
    /// patterns per rule source. User settings resolve at config home,
    /// flag settings at the flag file dirname, and project/local/policy
    /// at original cwd.
    pub permission_rule_source_roots:
        std::collections::HashMap<PermissionRuleSource, std::path::PathBuf>,
    /// Per-engine isolated rules to pre-seed into this engine's
    /// `live_command_rules` at construction (Command-source additions a
    /// subagent/skill carries that must NOT leak into the shared app_state
    /// base). Empty for the main session. See [`PermissionDerivation`].
    pub initial_command_rules: Vec<PermissionRule>,
    /// Per-engine permission derivation applied on top of the shared live
    /// base in `ToolContextFactory::build`. `None` = main session (identity).
    /// `Some` = subagent/fork/teammate: the Rust analog of TS
    /// `createSubagentContext`/`agentGetAppState` (read-through the parent base
    /// + `allowedTools` replace-on-restrict + extra read dirs + mode override).
    pub permission_derivation: Option<PermissionDerivation>,
    /// Working directory override for this session's tool calls.
    ///
    /// When `Some(path)`, [`ToolContextFactory`](crate::tool_context::ToolContextFactory)
    /// installs the path onto every `ToolUseContext.cwd_override` so
    /// file/shell/search tools that honor the override (Glob, Grep,
    /// Bash, and future worktree-aware tools) resolve relative paths
    /// against it. Absolute-path tools (Read, Write, Edit,
    /// NotebookEdit) are unaffected by construction — they enforce
    /// absolute paths in their schema.
    ///
    /// Phase 6 Workstream C: subagents launched with
    /// `isolation: "worktree"` receive a `cwd_override` pointing at
    /// the freshly-created worktree path via this field on their
    /// child `QueryEngineConfig`.
    pub cwd_override: Option<std::path::PathBuf>,
    /// Optional override for the plans directory, relative to the
    /// project root. Empty = use the default `~/.coco/plans/`.
    /// Validated by [`coco_context::resolve_plans_directory`] to stay
    /// within the project root.
    pub plans_directory: Option<String>,
    /// Set when this engine runs AS a subagent — the agent's branded ID.
    /// Threads into `ToolUseContext::agent_id` + `session_plan_file` so
    /// the subagent auto-allow targets `{slug}-agent-{id}.md` instead of
    /// the main `{slug}.md`, and so the per-turn plan reminder picks the
    /// SubAgent text variant. `None` = this engine IS the main session.
    pub agent_id: Option<String>,
    /// Set when this engine runs AS a swarm teammate (spawned via
    /// `TeamCreate` + in-process runner). Lifted to a config flag so
    /// `ToolUseContext.is_teammate` is set correctly without reading
    /// task-local state at every tool call.
    pub is_teammate: bool,
    /// True only for in-process swarm teammates. Pane teammates are
    /// separate CLI sessions and can manage background subagents.
    pub is_in_process_teammate: bool,
    /// Per-role `plan_mode_required` flag for teammates. Read from the
    /// role definition in the team file or `COCO_PLAN_MODE_REQUIRED`.
    /// When `true`, the teammate's ExitPlanMode MUST request leader
    /// approval via mailbox.
    /// When `false`, teammates exit locally (voluntary plan mode).
    /// Only meaningful when `is_teammate == true`.
    pub plan_mode_required: bool,
    /// Plan-mode workflow + prompt settings. Drives which Full reminder
    /// variant `PlanModeReminder` emits.
    pub plan_mode_settings: PlanModeSettings,
    /// Disable all hooks (from settings).
    pub disable_all_hooks: bool,
    /// Only allow managed/policy hooks (from settings).
    pub allow_managed_hooks_only: bool,
    /// Enable token-budget-driven turn continuation: when a turn ends naturally
    /// (no tool calls, `end_turn` stop) but consumed tokens are below 90% of
    /// `max_tokens` budget, inject a nudge meta message and continue.
    pub enable_token_budget_continuation: bool,
    /// Resolved compaction configuration (auto / micro / api-native /
    /// session-memory / experimental). Single source of truth — engine
    /// reads only this, never env directly. Env vars are folded in by
    /// `coco_config::CompactConfig::resolve` at startup.
    pub compact: coco_config::CompactConfig,
    /// Per-session raw-wire debug dumper. `Some` only when
    /// `diagnostics.wire_dump` is `error`/`all`; built by the bootstrap
    /// with the session's `wire/` directory. `None` means no capture and
    /// zero overhead.
    pub wire_dump: Option<coco_wire_dump::WireDumpConfig>,
    /// System-reminder subsystem configuration (per-generator toggles,
    /// timeout, critical-instruction payload). Bootstrap reads
    /// `settings.system_reminder` from `coco-config::Settings` and
    /// threads it through here so the engine can run `settings.json`
    /// through to every reminder generator without extra glue code.
    pub system_reminder: coco_config::SystemReminderConfig,
    /// Resolved tool runtime configuration.
    pub tool_config: ToolConfig,
    /// Resolved sandbox runtime configuration. User-facing settings only;
    /// for actual enforcement see [`Self::sandbox_state`].
    pub sandbox_config: SandboxSettings,
    /// Active sandbox runtime state. `None` when sandbox is disabled or
    /// not bootstrapped (test/headless paths). The CLI bootstrap layer
    /// constructs this via `coco_sandbox::adapter::build_runtime_config`
    /// and threads it onto `ToolUseContext.sandbox_state`.
    pub sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
    /// Resolved memory runtime configuration.
    pub memory_config: MemoryConfig,
    /// Resolved shell runtime configuration (bash-tool path).
    pub shell_config: ShellConfig,
    /// Model-facing shell tool selected for this session.
    pub active_shell_tool: ActiveShellTool,
    /// Session-scoped shell command assembler. Constructed once at
    /// session bootstrap (with the live snapshot watch + session-env
    /// reader + `/env` store) and threaded onto every
    /// [`coco_tool_runtime::ToolUseContext`]. `None` for tests / SDK
    /// paths that haven't yet wired the provider — Bash falls back to
    /// per-call executor construction without snapshot caching.
    pub shell_provider: Option<Arc<dyn coco_shell::ShellProvider>>,
    /// Frozen anchor — captured at session start. BashTool's
    /// `reset_cwd_if_outside_project` uses it to snap back when the
    /// live cwd drifts out of the allowed working set.
    pub original_cwd: Option<std::path::PathBuf>,
    /// Mutable session CWD shared across all BashTool invocations.
    /// `cd /tmp` in turn N updates this; turn N+1 reads it as the
    /// spawn cwd. `None` for tests / SDK paths; BashTool falls back
    /// to `std::env::current_dir()`.
    pub session_cwd: Option<Arc<tokio::sync::RwLock<std::path::PathBuf>>>,
    /// Resolved web-fetch runtime configuration (WebFetchTool).
    pub web_fetch_config: WebFetchConfig,
    /// Resolved web-search runtime configuration (WebSearchTool).
    pub web_search_config: WebSearchConfig,
    /// Resolved LSP tool-layer runtime configuration (LspTool).
    /// Carries the per-query file-size gate; future fields land here.
    pub lsp_config: coco_config::LspConfig,
    /// Centralized feature gates (Layer 1 of the tool filter pipeline).
    /// See `docs/coco-rs/feature-gates-and-tool-filtering.md`.
    pub features: Arc<Features>,
    /// Per-tier `skill_overrides` map preserved without merging.
    /// Threaded through the per-turn reminder pipeline and the
    /// SkillTool gate so the model only sees what the user permitted.
    /// Default-empty tiers ⇒ every skill resolves to `on` ⇒ all gates
    /// no-op. See `coco_config::SkillOverrideTiers`.
    pub skill_overrides: Arc<coco_config::SkillOverrideTiers>,
    /// Layer 2 — extra tools the active model adds + baseline tools it
    /// excludes.
    pub tool_overrides: Arc<ToolOverrides>,
    /// Layer 4 — agent-level allow/deny list. Top-level sessions use
    /// `unrestricted()`; subagents narrow it from `AgentDefinition`.
    pub tool_filter: ToolFilter,
    /// Sandboxed write fence — FileWrite / FileEdit / NotebookEdit may
    /// only target paths under one of these roots. Empty = no fence.
    /// Set on subagents launched by the memory crate (extraction /
    /// auto-dream) so the child can only write inside the memdir.
    /// Threaded onto every `ToolUseContext.allowed_write_roots`.
    pub allowed_write_roots: Vec<std::path::PathBuf>,
    /// Emit `HookExecutionEvent` (`Started`/`Progress`/`Response`) into
    /// the SDK output stream (CLI `--include-hook-events`). When `false`,
    /// the engine bypasses the hook-event forwarding channel so SDK
    /// clients don't receive `SDKHookStarted`/etc. messages. Defaults
    /// to `false`.
    pub include_hook_events: bool,
    /// Per-fork tool-execution gate. When `Some`, the engine threads
    /// the handle onto every `ToolUseContext` it builds, so the
    /// tool-call preparer runs the callback before the static
    /// permission evaluator. `None` preserves pre-canUseTool-wiring
    /// behavior.
    pub can_use_tool: Option<coco_tool_runtime::CanUseToolHandleRef>,

    /// Override label returned by [`crate::engine_builder::query_source_label`].
    /// When `Some`, the engine reports this string instead of the
    /// agent_id / non-interactive / repl_main_thread default. Forks
    /// pass their `ForkedAgentOptions.query_source` through here so
    /// telemetry can split traffic per-fork.
    pub query_source_override: Option<String>,

    /// Typed fork discriminator. Threaded into
    /// [`crate::engine_session::run_internal_with_messages`]'s session-loop
    /// `info!` macro so log lines self-identify which fork they belong
    /// to.
    pub fork_label: Option<coco_types::ForkLabel>,

    /// Sub-context isolation overrides. `Some` ⇒ the engine is
    /// fork-spawned and the per-call `ToolUseContext` builder
    /// applies the [`ForkContextOverrides`] field-by-field
    /// (auto agent_id, query_depth bump, allowed_write_roots fence,
    /// isolated callback handles). `None` ⇒ standard parent-shared
    /// semantics (default).
    ///
    /// The `clone_file_read_state` flag is honored at engine-build time
    /// (`SessionRuntime::build_engine_from_config_with_persistence` installs
    /// a deep-cloned `FileReadState` so the clone isn't repeated per-call);
    /// the remaining fields apply at per-call `ToolUseContext` construction.
    ///
    /// Stored as `Arc` so threading it onto every per-call
    /// `ToolUseContext` is a cheap pointer-copy.
    pub fork_isolation: Option<std::sync::Arc<crate::fork_context::ForkContextOverrides>>,
}

impl Default for QueryEngineConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            total_token_budget: None,
            prompt_cache: None,
            system_prompt: None,
            append_system_prompt: None,
            model_id: String::new(),
            permission_mode: PermissionMode::Default,
            bypass_permissions_available: false,
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_output_tokens: 16_384,
            max_budget_usd: None,
            output_token_budget: None,
            // Phase 9 landed: safe tools start mid-stream via
            // StreamingHandle, unsafe tools queue for commit_flush.
            // Default ON — the batched-at-end fallback path stays
            // available by setting this to `false`.
            streaming_tool_execution: true,
            is_non_interactive: false,
            avoid_permission_prompts: false,
            debug: false,
            verbose: false,
            thinking_level: None,
            fast_mode: false,
            session_id: String::new(),
            project_dir: None,
            live_permission_rules: None,
            live_permission_mode: None,
            permission_rule_source_roots: Default::default(),
            initial_command_rules: Vec::new(),
            permission_derivation: None,
            cwd_override: None,
            plans_directory: None,
            agent_id: None,
            is_teammate: false,
            is_in_process_teammate: false,
            plan_mode_required: false,
            plan_mode_settings: PlanModeSettings::default(),
            disable_all_hooks: false,
            allow_managed_hooks_only: false,
            enable_token_budget_continuation: false,
            compact: coco_config::CompactConfig::default(),
            wire_dump: None,
            system_reminder: coco_config::SystemReminderConfig::default(),
            tool_config: ToolConfig::default(),
            sandbox_config: SandboxSettings::default(),
            sandbox_state: None,
            memory_config: MemoryConfig::default(),
            shell_config: ShellConfig::default(),
            // Engine tests and SDK-style harnesses build QueryEngineConfig
            // directly, bypassing CLI session bootstrap where shell.tool=auto
            // resolves to the platform default. Preserve the historical Bash
            // visibility for those callers; fork/spawn request DTOs still
            // default to Disabled until a parent explicitly threads a value.
            active_shell_tool: ActiveShellTool::Bash,
            shell_provider: None,
            original_cwd: None,
            session_cwd: None,
            web_fetch_config: WebFetchConfig::default(),
            web_search_config: WebSearchConfig::default(),
            lsp_config: coco_config::LspConfig::default(),
            features: Arc::new(Features::with_defaults()),
            skill_overrides: Arc::new(coco_config::SkillOverrideTiers::default()),
            tool_overrides: Arc::new(ToolOverrides::none()),
            tool_filter: ToolFilter::unrestricted(),
            allowed_write_roots: Vec::new(),
            include_hook_events: false,
            can_use_tool: None,
            query_source_override: None,
            fork_label: None,
            fork_isolation: None,
        }
    }
}

impl QueryEngineConfig {
    /// Convenience: whether auto-compaction is currently allowed (user
    /// toggle AND env kill switches resolved). Used by the system-reminder
    /// generator and the auto-compact branch in `finalize_turn_post_tools`.
    #[must_use]
    pub fn is_auto_compact_active(&self) -> bool {
        self.compact.auto.is_active()
    }
}

/// One-shot bootstrap data for `SessionStarted` emission.
///
/// Collected by the CLI layer at session start and handed to the engine so it
/// can emit a single `CoreEvent::Protocol(ServerNotification::SessionStarted)`
/// with full context before the first turn.
#[derive(Debug, Clone, Default)]
pub struct SessionBootstrap {
    pub protocol_version: String,
    pub cwd: String,
    pub version: String,
    /// Tool names the LLM will see. If empty, the engine falls back to
    /// `ToolRegistry::loaded_tools()` names.
    pub tools: Vec<String>,
    pub slash_commands: Vec<String>,
    pub agents: Vec<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<coco_types::McpServerInit>,
    pub plugins: Vec<coco_types::PluginInit>,
    pub api_key_source: Option<String>,
    pub betas: Vec<String>,
    pub output_style: Option<String>,
    pub fast_mode_state: Option<coco_types::FastModeState>,
}

/// Result of running the query engine.
#[derive(Debug)]
pub struct QueryResult {
    /// Final assistant text response.
    pub response_text: String,
    /// Total turns executed.
    pub turns: i32,
    /// Accumulated token usage.
    pub total_usage: TokenUsage,
    /// Per-model cost tracking.
    pub cost_tracker: CostTracker,
    /// Whether the engine was cancelled.
    pub cancelled: bool,
    /// Whether the budget was exhausted.
    pub budget_exhausted: bool,
    /// Why the engine stopped (last continue reason or None for clean exit).
    pub last_continue_reason: Option<ContinueReason>,
    /// Total duration in milliseconds.
    pub duration_ms: i64,
    /// Total API time in milliseconds.
    pub duration_api_ms: i64,
    /// Stop reason from the model.
    pub stop_reason: Option<String>,
    /// Permission denials accumulated during the session. Populated on each
    /// `PermissionDecision::Deny` branch in the tool execution loop and
    /// flushed into `SessionResultParams` at session end.
    pub permission_denials: Vec<coco_types::PermissionDenialInfo>,
    /// Final message history at the end of the turn, including the
    /// user prompt, any tool calls + results, and the final assistant
    /// reply. Used by multi-turn SDK sessions to thread context
    /// forward into the next `turn/start`, and by the TUI / SDK
    /// runners to write-back into `SessionRuntime.history` via
    /// `MessageHistory::push_arc` — no deep `Message` clone, no
    /// re-Arc allocation across the engine→runtime seam (plan §11
    /// F8 follow-up).
    pub final_messages: Vec<std::sync::Arc<Message>>,
    /// Same final transcript as [`Self::final_messages`], preserving
    /// `MessageHistory` state such as the latest provider usage marker.
    pub final_history: MessageHistory,
    /// Structured output captured from a `structured_output` attachment.
    pub structured_output: Option<serde_json::Value>,
    /// Max-turn terminal signal captured from this engine invocation.
    pub max_turns_reached: Option<coco_types::MaxTurnsReachedPayload>,
}
