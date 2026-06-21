//! `ToolContextFactory` — single owner of `ToolUseContext` construction.
//!
//! One factory owns the field mapping from the `query()` loop's shared
//! state, and the engine asks for a fresh context per turn.
//!
//! The factory exists so the refactor plan's I6 invariant (ToolUseContext is
//! accurate, not hardcoded defaults) is enforceable by tests — callers can
//! construct a factory from a subset of engine state and verify the fields
//! propagate without spinning up a full `QueryEngine`.
//!
//! # Fields fixed by this factory (I6)
//!
//! - `thinking_level`, `is_non_interactive`, `max_budget_usd`,
//!   `custom_system_prompt`, `append_system_prompt` — previously hardcoded
//!   defaults in `engine.rs`. Now mirrored from `QueryEngineConfig`.
//! - `main_loop_model` snapshots the currently-active model. The engine
//!   passes `ToolContextOverrides.current_model_id` at build time from
//!   `ModelRuntime::current_model_id()` so post-fallback contexts reflect
//!   the active slot. Callers that omit the override (tests, legacy single-
//!   client paths) fall back to `config.model_id`.
//! - `hook_handle` — plumbed through even when `None` so Phase 3's
//!   `QueryHookHandle` slots in without a second call-site edit.
//! - Permission-mode-related fields (`mode`, `pre_plan_mode`,
//!   `stripped_dangerous_rules`) are seeded from live `ToolAppState`
//!   when present so mid-session mutations (e.g. `EnterPlanMode`) are
//!   visible on the next batch.

use std::path::PathBuf;
use std::sync::Arc;

use coco_config::EnvKey;
use coco_config::env;
use coco_context::FileHistoryState;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::HookHandleRef;
use coco_tool_runtime::LspHandleRef;
use coco_tool_runtime::MailboxHandleRef;
use coco_tool_runtime::McpHandleRef;
use coco_tool_runtime::ScheduleStoreRef;
use coco_tool_runtime::SkillHandleRef;
use coco_tool_runtime::TaskListHandleRef;
use coco_tool_runtime::TeamTaskListRouterRef;
use coco_tool_runtime::TodoListHandleRef;
use coco_tool_runtime::ToolAbortSignal;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::TurnAbortSignal;
use coco_types::AgentId;
use coco_types::AppStateReadHandle;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRulesBySource;
use coco_types::ToolAppState;
use coco_types::ToolPermissionContext;
use tokio::sync::RwLock;

use crate::config::QueryEngineConfig;

/// Immutable inputs needed to build a `ToolUseContext`.
///
/// Carries the QueryEngine state relevant to tool execution,
/// minus per-call overrides (which go through [`ToolContextOverrides`]).
///
/// All `Arc`/`Option<Arc<_>>` fields are cheap to clone; the factory keeps them
/// by value so a caller can construct one factory and reuse it across turns.
pub(crate) struct ToolContextFactory {
    pub(crate) config: QueryEngineConfig,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) turn_abort: TurnAbortSignal,
    pub(crate) mailbox: Option<MailboxHandleRef>,
    /// In-memory FIFO of per-recipient pending messages. Production
    /// wires the same `Arc<InMemoryPendingMessageStore>` here AND on the
    /// `SwarmAdapter` reminder source so SendMessage push → reminder
    /// drain stays a closed loop. `None` falls back to a no-op store.
    pub(crate) pending_messages: Option<coco_tool_runtime::PendingMessageStoreRef>,
    pub(crate) task_list: Option<TaskListHandleRef>,
    pub(crate) team_task_list_router: Option<TeamTaskListRouterRef>,
    pub(crate) todo_list: Option<TodoListHandleRef>,
    pub(crate) task_handle: Option<coco_tool_runtime::BackgroundTaskHandleRef>,
    pub(crate) permission_bridge: Option<ToolPermissionBridgeRef>,
    pub(crate) app_state: Option<Arc<RwLock<ToolAppState>>>,
    pub(crate) file_read_state: Option<Arc<RwLock<coco_context::FileReadState>>>,
    pub(crate) file_history: Option<Arc<RwLock<FileHistoryState>>>,
    pub(crate) config_home: Option<PathBuf>,
    pub(crate) tool_result_session_dir: Option<PathBuf>,
    pub(crate) transcript_path: Option<PathBuf>,
    /// Optional hook-callback bridge. `None` in tests and phases where the
    /// adapter is not yet installed; filled in by Phase 3's `QueryHookHandle`.
    pub(crate) hook_handle: Option<HookHandleRef>,
    /// Optional agent-runtime handle. `None` resolves to
    /// `NoOpAgentHandle`, which returns "not available" errors for
    /// every `AgentTool` subcommand — suitable for tests and
    /// sessions that intentionally disable subagent spawning. The
    /// CLI / SDK / TUI runners install a real handle (the
    /// swarm-backed `SwarmAgentHandle`) at session bootstrap so
    /// `ctx.agent.spawn_agent(...)` reaches the real runtime.
    pub(crate) agent_handle: Option<AgentHandleRef>,
    /// Optional skill-runtime handle. `None` resolves to
    /// `NoOpSkillHandle`, which returns `Unavailable` from every
    /// `invoke_skill` call — the model sees a clean error instead
    /// of silent skipping. Swap in a real handle once `SkillRuntime`
    /// implementations land (Phase 7-β).
    pub(crate) skill_handle: Option<SkillHandleRef>,
    /// Optional LSP-runtime handle. `None` resolves to
    /// `NoOpLspHandle`, whose `is_connected() = false` hides
    /// [`LspTool`](coco_tools::LspTool) from the model's tool list.
    /// CLI / SDK / TUI runners install a real handle (the
    /// `LspManagerAdapter`) at session bootstrap when
    /// `Feature::Lsp` is enabled.
    pub(crate) lsp_handle: Option<LspHandleRef>,
    /// Optional MCP-runtime handle. `None` resolves to
    /// `NoOpMcpHandle`. Without a real handle installed,
    /// `McpAuthTool` / `ListMcpResourcesTool` / `ReadMcpResourceTool`
    /// / dynamic `McpTool` wrappers degrade to "no MCP available"
    /// errors. CLI / SDK runners install
    /// `mcp_handle_adapter::McpManagerAdapter`; TUI currently passes
    /// `None` (no MCP bootstrap yet in TUI runner).
    pub(crate) mcp_handle: Option<McpHandleRef>,
    /// Optional scheduling backend. `None` resolves to `NoOpScheduleStore`.
    pub(crate) schedule_store: Option<ScheduleStoreRef>,
    /// Active agent-definition catalog snapshot (T7). Surfaced on
    /// `ToolUseContext.agent_catalog` so AgentTool can resolve a
    /// `subagent_type` to its full `AgentDefinition` and thread the
    /// definition through `AgentSpawnRequest.definition`. Built by
    /// the session bootstrap from `coco_subagent::AgentDefinitionStore`
    /// and refreshed on `/agents reload`. `None` is the legacy/test
    /// path — AgentTool degrades to subagent_type→role mapping alone.
    pub(crate) agent_catalog: Option<Arc<coco_subagent::AgentCatalogSnapshot>>,
    /// Parent's resolved runtime identity (provider + API + model). Set
    /// from the runtime registry snapshot and threaded onto every
    /// `ToolUseContext` so subagent spawns can pin Fork-mode prompt cache
    /// to the parent's model. `None` is the legacy/test path; AgentTool
    /// degrades to coordinator's `current_main_model_id()` which can
    /// drift on hot-reload.
    pub(crate) parent_runtime_snapshot: Option<Arc<coco_types::SubagentRuntimeSnapshot>>,
    /// Per-engine skill-emitted Command-source rule store, shared by
    /// `Arc` with `QueryEngine.live_command_rules` and the
    /// `EngineLiveRulesHandle` installed on the executor.
    ///
    /// At each batch's `build()`, the factory `read()`s this Arc and
    /// merges its contents into the returned context's
    /// `permission_context.allow_rules[Command]` so the evaluator sees
    /// rules emitted by prior turns of the same user message. The Arc
    /// drops with the engine — see [`crate::engine_live_rules`].
    pub(crate) live_command_rules: Arc<RwLock<Vec<PermissionRule>>>,
}

/// Per-call overrides applied on top of [`ToolContextFactory`] inputs.
#[derive(Default)]
pub(crate) struct ToolContextOverrides {
    /// UUID of the user message that drove this turn; threaded through so
    /// `file_history` snapshots key on the correct message.
    pub(crate) user_message_id: Option<String>,
    /// Per-turn progress channel. Tools (e.g. `Bash`) clone this via
    /// `ctx.progress_tx` and push `ToolProgress` updates; the engine
    /// spawns a drain task that forwards those updates to `event_tx`
    /// as `TuiOnlyEvent::ToolProgress` events.
    ///
    /// `None` ⇒ tools run without progress reporting.
    pub(crate) progress_tx: Option<coco_tool_runtime::ProgressSender>,
    /// Currently-active model name. Engine passes
    /// `ModelRuntime::current_model_id()` so `main_loop_model`
    /// reflects post-fallback state; absent falls back to
    /// `config.model_id` (tests, pre-fallback paths).
    pub(crate) current_model_id: Option<String>,
    /// `true` when the post-fallback active model declares
    /// [`coco_types::Capability::ServerSideToolReference`]. Engine
    /// resolves this from the active runtime snapshot so a model swap
    /// (primary → fallback) changes the ToolSearch envelope shape
    /// without a context-factory rebuild. Default `false` keeps the
    /// non-Anthropic / non-capable path (client-side promotion).
    pub(crate) current_model_supports_tool_reference: bool,
    /// `true` when the active model declares
    /// [`coco_types::Capability::ClientSideToolSearch`] — the
    /// universal `discovered_tool_names` promotion path. Default
    /// `false` for unknown/custom models so they degrade to
    /// eager-loading (no `ToolSearch` round-trip).
    ///
    /// Combined with [`Self::current_model_supports_tool_reference`]
    /// inside the factory to populate the ctx capability flags;
    /// `ToolUseContext::tool_search_active()` then drives the
    /// runtime three-state activation.
    pub(crate) current_model_supports_client_side_tool_search: bool,
    /// Post-budget message snapshot from `build_prompt`. Threaded onto
    /// `ToolUseContext.messages` so tools observe the exact view this
    /// turn's model just received. `None` falls back to an empty
    /// snapshot for test stubs / pre-first-turn paths.
    pub(crate) messages_snapshot:
        Option<std::sync::Arc<Vec<std::sync::Arc<coco_messages::Message>>>>,
}

fn merge_rules_by_behavior(
    target: &mut PermissionRulesBySource,
    live_rules: &[PermissionRule],
    behavior: PermissionBehavior,
) {
    for rule in live_rules
        .iter()
        .filter(|rule| rule.behavior == behavior)
        .cloned()
    {
        target.entry(rule.source).or_default().push(rule);
    }
}

impl ToolContextFactory {
    /// Build a fresh `ToolUseContext` for the next tool batch.
    ///
    /// Reads permission-mode-related fields from `app_state` when available —
    /// the prior batch's `ExitPlanModeTool` / `EnterPlanModeTool` patches
    /// become visible here without a config reload.
    pub(crate) async fn build(&self, overrides: ToolContextOverrides) -> ToolUseContext {
        // Snapshot the WHOLE live permission base (mode + pre_plan + stripped +
        // allow/deny/ask + additional_dirs + source_roots) in ONE read-lock
        // acquire, plus the discovered-tool names. Lock discipline: clone out,
        // drop the guard before any `await`. `ToolAppState` is the single live
        // source of truth (TS `appState.toolPermissionContext`); subagents share
        // the parent's Arc so they read-through the parent's live rules.
        //
        // Fallback (no app_state): test/legacy engines derive the base from the
        // config snapshot. (S6 removes the config rule fields once every engine
        // carries an app_state.)
        let (mut base, live_discovered_tool_names) = match self.app_state.as_ref() {
            Some(state) => {
                let guard = state.read().await;
                (
                    guard.permissions.clone(),
                    std::sync::Arc::new(guard.discovered_tool_names.clone()),
                )
            }
            None => (
                coco_types::LiveToolPermissionState {
                    mode: Some(self.config.permission_mode),
                    permission_rule_source_roots: self.config.permission_rule_source_roots.clone(),
                    ..Default::default()
                },
                std::sync::Arc::new(std::collections::HashSet::new()),
            ),
        };
        let mut live_mode = base.mode.unwrap_or(self.config.permission_mode);
        let live_pre_plan = base.pre_plan_mode;
        let live_stripped = base.stripped_dangerous_rules.clone();
        if let Some(mode) = self.config.live_permission_mode.as_ref() {
            live_mode = *mode.read().await;
        }

        // Per-engine permission derivation (subagent / fork / teammate): the
        // Rust analog of TS `createSubagentContext` + `agentGetAppState`. Reads
        // through the shared base (deny/ask already inherited above) and layers
        // the per-engine deltas WITHOUT mutating the shared app_state. `None`
        // for the main session (identity).
        if let Some(deriv) = self.config.permission_derivation.as_ref() {
            // Agent-definition mode override, but never widen past a parent
            // Bypass/AcceptEdits/Auto (TS `runAgent.ts:421-427`).
            if let Some(om) = deriv.mode_override
                && !matches!(
                    live_mode,
                    coco_types::PermissionMode::BypassPermissions
                        | coco_types::PermissionMode::AcceptEdits
                        | coco_types::PermissionMode::Auto
                )
            {
                live_mode = om;
            }
            // `allowedTools` replace-on-restrict: keep only the parent's
            // CliArg-source allow + the explicit allowed tools as Session-source
            // allow, dropping the parent's other allow sources. deny/ask remain
            // fully inherited (TS `runAgent.ts:469-479`).
            if let Some(replace) = deriv.allowed_tools_replace.as_ref() {
                let cli_arg = base.allow_rules.remove(&PermissionRuleSource::CliArg);
                base.allow_rules.clear();
                if let Some(rules) = cli_arg {
                    base.allow_rules.insert(PermissionRuleSource::CliArg, rules);
                }
                if !replace.is_empty() {
                    base.allow_rules
                        .entry(PermissionRuleSource::Session)
                        .or_default()
                        .extend(replace.iter().cloned());
                }
            }
            // Extra read-scope dirs (parent cwd bridge + inherited dirs).
            for (k, v) in &deriv.extra_additional_dirs {
                base.additional_dirs.insert(k.clone(), v.clone());
            }

            // Verification: a subagent/fork shares the parent app_state Arc, so
            // `base` (snapshotted above) already carries the parent's
            // deny/ask/allow rules read-through — the derivation never strips
            // them. Log the inherited counts so the 85232fa4 fix (subagents
            // inherit parent deny/ask, closing the read-bypass) is observable.
            let deny_count: usize = base.deny_rules.values().map(Vec::len).sum();
            let ask_count: usize = base.ask_rules.values().map(Vec::len).sum();
            let allow_count: usize = base.allow_rules.values().map(Vec::len).sum();
            tracing::debug!(
                target: "coco_query::tool_context",
                agent_id = ?self.config.agent_id,
                inherited_deny_rules = deny_count,
                inherited_ask_rules = ask_count,
                inherited_allow_rules = allow_count,
                inherited_additional_dirs = base.additional_dirs.len(),
                mode = ?live_mode,
                "tool_context: subagent permission derivation — inherited parent \
                 deny/ask/allow from shared app_state base",
            );
        }

        // Plan-mode paths resolve unconditionally: fall back to the global
        // config home when this engine wasn't handed one, so plan-file
        // reads/writes are exempt in every runtime (TUI / subagent / SDK /
        // headless). Always resolves a default rather than going dark when
        // no per-engine home is set.
        let config_home = self
            .config_home
            .clone()
            .unwrap_or_else(coco_config::global_config::config_home);
        let plans_directory = coco_context::resolve_plans_directory(
            &config_home,
            self.config.project_dir.as_deref(),
            self.config.plans_directory.as_deref(),
        );
        let session_plan_file = Some(coco_context::get_plan_file_path(
            &self.config.session_id,
            &plans_directory,
            self.config.agent_id.as_deref(),
        ));
        let plans_dir = Some(plans_directory);

        let main_loop_model = overrides
            .current_model_id
            .unwrap_or_else(|| self.config.model_id.clone());

        // Merge the per-engine live Command-source rules into the
        // batch-time `allow_rules` snapshot. `alwaysAllowRules.command`
        // is read at every permission check; we snapshot once per batch
        // (factory.build is called per batch). Cross-batch propagation
        // works because each turn's build() re-reads the live Arc. The
        // empty-fast-path avoids a clone when no skill has emitted rules yet.
        let live_permission_rules = match self.config.live_permission_rules.as_ref() {
            Some(rules) => rules.read().await.clone(),
            None => Vec::new(),
        };

        let mut allow_rules = {
            let live = self.live_command_rules.read().await;
            if live.is_empty() {
                // Hot path: factory builds every batch; emitting one
                // log per build at debug would dominate the file
                // sink. Stay silent here — info logs in
                // `engine_live_rules` already mark the meaningful
                // state transition (rules being added).
                base.allow_rules.clone()
            } else {
                let live_count = live.len();
                let base_command_count = base
                    .allow_rules
                    .get(&PermissionRuleSource::Command)
                    .map(Vec::len)
                    .unwrap_or(0);
                let mut merged = base.allow_rules.clone();
                merged
                    .entry(PermissionRuleSource::Command)
                    .or_default()
                    .extend(live.iter().cloned());
                let merged_command_count = merged
                    .get(&PermissionRuleSource::Command)
                    .map(Vec::len)
                    .unwrap_or(0);
                tracing::debug!(
                    session_id = %self.config.session_id,
                    live_count,
                    base_command_count,
                    merged_command_count,
                    "tool_context: merged live Command rules into allow_rules"
                );
                merged
            }
        };
        merge_rules_by_behavior(
            &mut allow_rules,
            &live_permission_rules,
            PermissionBehavior::Allow,
        );
        // Auto-mode classifier-bypass guard: physically remove dangerous
        // classifier-bypassing allow rules (`Bash(python:*)`, `Agent`, …) from
        // the evaluator-facing `allow_rules` whenever the live mode is Auto.
        // Without this, evaluate.rs step-2 returns Allow on a dangerous allow
        // rule BEFORE the classifier gate in tool_call_preparer.rs — a fail-OPEN
        // bypass. TS `stripDangerousPermissionsForAutoMode` removes them from
        // `alwaysAllowRules` on Auto entry; coco-rs applies the same filter at
        // build time keyed on mode, so it is mandatory regardless of which entry
        // path (SDK / bridge / startup / plan-exit) set Auto. `is_ant_user=false`
        // mirrors the non-ant external-user path.
        if live_mode == coco_types::PermissionMode::Auto {
            let _ = coco_permissions::strip_dangerous_allow_rules(
                &mut allow_rules,
                /*is_ant_user*/ false,
            );
        }
        // Diagnostic: snapshot of the file-read/edit allow rules visible to
        // THIS batch's permission context, sourced from the live
        // `ToolAppState.permissions` base (read-through each batch). Grep this
        // line to confirm an `Edit(...)`/`Read(...)` rule the user just approved
        // is present for a same-cycle Read. Enable with
        // `COCO_LOG=coco_query::tool_context=debug`.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let file_allow_rules: Vec<String> = allow_rules
                .iter()
                .flat_map(|(source, rules)| {
                    rules.iter().filter_map(move |r| {
                        let pattern = r.value.tool_pattern.as_str();
                        (pattern == coco_types::ToolName::Edit.as_str()
                            || pattern == coco_types::ToolName::Read.as_str())
                        .then(|| {
                            format!(
                                "{source:?}:{pattern}({})",
                                r.value.rule_content.as_deref().unwrap_or("*")
                            )
                        })
                    })
                })
                .collect();
            tracing::debug!(
                session_id = %self.config.session_id,
                file_allow_rules = ?file_allow_rules,
                "tool_context: file read/edit allow-rule snapshot for this batch \
                 (frozen at engine build; mid-cycle approvals land on the next build)",
            );
        }
        let mut deny_rules = base.deny_rules.clone();
        merge_rules_by_behavior(
            &mut deny_rules,
            &live_permission_rules,
            PermissionBehavior::Deny,
        );
        let mut ask_rules = base.ask_rules.clone();
        merge_rules_by_behavior(
            &mut ask_rules,
            &live_permission_rules,
            PermissionBehavior::Ask,
        );
        let mut ctx = ToolUseContext {
            tools: self.tools.clone(),
            main_loop_model,
            // Honor the config-driven values that the previous inline
            // constructor hardcoded.
            thinking_level: self.config.thinking_level.clone(),
            is_non_interactive: self.config.is_non_interactive,
            avoid_permission_prompts: self.config.avoid_permission_prompts,
            max_budget_usd: self.config.max_budget_usd,
            custom_system_prompt: self.config.system_prompt.clone(),
            append_system_prompt: self.config.append_system_prompt.clone(),
            debug: self.config.debug,
            verbose: self.config.verbose,
            tool_config: self.config.tool_config.clone(),
            sandbox_config: self.config.sandbox_config.clone(),
            sandbox_state: self.config.sandbox_state.clone(),
            memory_config: self.config.memory_config.clone(),
            shell_config: self.config.shell_config.clone(),
            active_shell_tool: self.config.active_shell_tool,
            shell_provider: self.config.shell_provider.clone(),
            original_cwd: self.config.original_cwd.clone(),
            session_cwd: self.config.session_cwd.clone(),
            web_fetch_config: self.config.web_fetch_config.clone(),
            web_search_config: self.config.web_search_config.clone(),
            plan_mode_settings: self.config.plan_mode_settings.clone(),
            lsp_config: self.config.lsp_config.clone(),
            features: self.config.features.clone(),
            skill_overrides: self.config.skill_overrides.clone(),
            tool_overrides: self.config.tool_overrides.clone(),
            tool_filter: self.config.tool_filter.clone(),
            discovered_tool_names: live_discovered_tool_names,
            model_supports_tool_reference: overrides.current_model_supports_tool_reference,
            model_supports_client_side_tool_search: overrides
                .current_model_supports_client_side_tool_search,
            tool_search_has_candidates: false,
            is_teammate: self.config.is_teammate,
            is_in_process_teammate: self.config.is_in_process_teammate,
            plan_mode_required: self.config.plan_mode_required,
            // Pre-resolve swarm identity once, so tools read from ctx
            // instead of process env. Falls back to env vars set by the
            // teammate spawner for cross-process scenarios. Env namespace
            // is `COCO_*` — see swarm_constants.
            agent_name: env::env_opt(EnvKey::CocoAgentName)
                .or_else(|| self.config.agent_id.clone()),
            team_name: env::env_opt(EnvKey::CocoTeamName),
            plan_verify_execution: self.config.plan_mode_settings.verify_execution,
            // `isPlanModeInterviewPhaseEnabled()` is settings-only
            // (no Growthbook, no env var). Drives the EnterPlanMode
            // post-execute instruction-text variant.
            is_plan_interview_phase: matches!(
                self.config.plan_mode_settings.workflow,
                coco_config::PlanModeWorkflow::Interview
            ),
            abort: ToolAbortSignal::from_turn(self.turn_abort.clone()),
            messages: overrides
                .messages_snapshot
                .unwrap_or_else(|| Arc::new(Vec::new())),
            permission_context: ToolPermissionContext {
                mode: live_mode,
                // Per-session additional dirs (`/add-dir <path>` + inherited
                // read scope) read live from the shared base, so mid-session
                // widening is visible without an engine rebuild.
                additional_dirs: base.additional_dirs.clone(),
                // Permission rules from settings.json (user /
                // project / policy) merged with the per-engine live
                // Command-source rules emitted by skills earlier this
                // user message. Base rules plus the live delta both
                // read through the same evaluator slot.
                // Plan Tier 3 polish.
                allow_rules,
                deny_rules,
                ask_rules,
                bypass_available: self.config.bypass_permissions_available,
                pre_plan_mode: live_pre_plan,
                stripped_dangerous_rules: live_stripped,
                session_plan_file,
                permission_rule_source_roots: base.permission_rule_source_roots.clone(),
            },
            tool_use_id: None,
            user_message_id: overrides.user_message_id,
            // Fork isolation: when fork_isolation is set and the
            // config didn't pre-supply an agent_id, auto-gen one
            // (a fresh agentId is always allocated per fork).
            agent_id: self
                .config
                .agent_id
                .clone()
                .or_else(|| {
                    self.config.fork_isolation.as_ref().map(|iso| {
                        iso.agent_id
                            .clone()
                            .unwrap_or_else(|| crate::fork_context::auto_agent_id(iso.fork_label))
                    })
                })
                .as_ref()
                .map(AgentId::new),
            agent_type: None,
            // T7: agent catalog snapshot. Filled when the session
            // bootstrap calls `ToolContextFactory::with_agent_catalog`;
            // `None` resolves AgentTool to the subagent_type→role
            // mapping alone (legacy / test path).
            agent_catalog: self.agent_catalog.clone(),
            // Missed-2 fix: parent runtime snapshot threaded onto every
            // ToolUseContext. AgentTool reads this and populates
            // `AgentSpawnRequest.parent_runtime_snapshot` so fork-mode
            // prompt-cache parity survives `RuntimeConfig` hot-reload.
            // Installed at engine bootstrap via
            // `ToolContextFactory::with_parent_runtime_snapshot` from
            // the runtime registry snapshot.
            parent_runtime_snapshot: self.parent_runtime_snapshot.clone(),
            file_reading_limits: Default::default(),
            glob_limits: Default::default(),
            nested_memory_attachment_triggers: Arc::new(RwLock::new(Default::default())),
            loaded_nested_memory_paths: Default::default(),
            dynamic_skill_dir_triggers: Arc::new(RwLock::new(Default::default())),
            dynamic_skill_path_triggers: Arc::new(RwLock::new(Default::default())),
            discovered_skill_names: Default::default(),
            tool_decisions: Default::default(),
            user_modified: false,
            // Fork isolation honors `require_can_use_tool` flag —
            // speculation needs this so overlay path-rewrites always
            // run regardless of hook auto-approve config.
            require_can_use_tool: self
                .config
                .fork_isolation
                .as_ref()
                .map(|iso| iso.require_can_use_tool)
                .unwrap_or(false),
            preserve_tool_use_results: false,
            rendered_system_prompt: None,
            critical_system_reminder: None,
            in_progress_tool_use_ids: Arc::new(RwLock::new(Default::default())),
            side_query: Arc::new(coco_tool_runtime::NoOpSideQuery),
            mcp: self
                .mcp_handle
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpMcpHandle)),
            lsp: self
                .lsp_handle
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpLspHandle)),
            schedules: self
                .schedule_store
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpScheduleStore)),
            agent: self
                .agent_handle
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpAgentHandle)),
            skill: self
                .skill_handle
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpSkillHandle)),
            mailbox: self
                .mailbox
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpMailboxHandle)),
            pending_messages: self
                .pending_messages
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpPendingMessageStore)),
            // Phase 6 Workstream C hook: worktree-isolated subagents
            // receive `cwd_override` via their child engine config so
            // relative-path-resolving tools (Glob/Grep/Bash) operate
            // inside the worktree. Absolute-path tools ignore this
            // field by design (schema enforces absolute paths),
            // matching TS behavior where `AsyncLocalStorage`-based
            // cwd override only affects resolving calls.
            cwd_override: self.config.cwd_override.clone(),
            // Memdir-only write fence for sandboxed subagents (memory
            // extraction / auto-dream). Empty when the parent session
            // didn't install one. Fork isolation can override this
            // per-fork (e.g. memory services pin to memory_dir).
            allowed_write_roots: self
                .config
                .fork_isolation
                .as_ref()
                .filter(|iso| !iso.allowed_write_roots.is_empty())
                .map(|iso| iso.allowed_write_roots.clone())
                .unwrap_or_else(|| self.config.allowed_write_roots.clone()),
            permission_bridge: self.permission_bridge.clone(),
            // Per-fork canUseTool callback. Threaded from
            // QueryEngineConfig (set by ForkedAgentOptions →
            // fork_dispatcher) onto every ToolUseContext built for
            // this engine, so the preparer gates every tool call
            // before static permission evaluation.
            can_use_tool: self.config.can_use_tool.clone(),
            progress_tx: overrides.progress_tx,
            task_handle: self.task_handle.clone(),
            task_list: self
                .task_list
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::NoOpTaskListHandle)),
            team_task_list_router: self.team_task_list_router.clone(),
            todo_list: self
                .todo_list
                .clone()
                .unwrap_or_else(|| Arc::new(coco_tool_runtime::InMemoryTodoListHandle::new())),
            hook_handle: self.hook_handle.clone(),
            file_read_state: self.file_read_state.clone(),
            file_history: self.file_history.clone(),
            config_home: self.config_home.clone(),
            session_id_for_history: Some(self.config.session_id.clone()),
            tool_result_session_dir: self.tool_result_session_dir.clone(),
            transcript_path: self.transcript_path.clone(),
            approval_feedback: None,
            permission_resolution_detail: None,
            plans_dir,
            app_state: self
                .app_state
                .as_ref()
                .map(|arc| AppStateReadHandle::new(arc.clone())),
            // Per-subagent denial isolation (TS `createSubagentContext`
            // parity): every subagent — fork OR ordinary — gets its own fresh
            // `DenialTracker`, so one child's consecutive-denial streak cannot
            // trip the SHARED auto-mode circuit breaker for the parent and its
            // sibling subagents (which, under parallel spawns, would cascade
            // every concurrent agent back to manual prompts). TS does exactly
            // this: `localDenialTracking: createDenialTrackingState()` for any
            // subagent context, sharing only when `shareSetAppState`. The main
            // session (`agent_id == None`) keeps `None` here and uses the
            // engine-level session tracker. `AutoModeState` stays shared — only
            // the per-agent denial *counter* is isolated. The classifier site
            // honors this by reading `ctx.local_denial_tracking` before the
            // engine-level session tracker.
            local_denial_tracking: self.config.agent_id.as_ref().map(|_| {
                Arc::new(tokio::sync::Mutex::new(
                    coco_tool_runtime::DenialTracker::new(),
                ))
            }),
            // Query-tracking chain id: forks start a fresh UUID so
            // telemetry can group fork traffic separately from main
            // loop.
            query_chain_id: self
                .config
                .fork_isolation
                .as_ref()
                .map(|_| uuid::Uuid::new_v4().to_string()),
            // Query-tracking depth: parent depth + 1 (capped at 16).
            query_depth: self
                .config
                .fork_isolation
                .as_ref()
                .map(|iso| iso.child_query_depth())
                .unwrap_or(0),
        };

        if ctx.tool_search_supported() {
            ctx.tool_search_has_candidates = true;
            let has_deferred = !ctx.tools.deferred_tools(&ctx).is_empty();
            let has_pending_mcp = !ctx.mcp.pending_server_names().await.is_empty();
            ctx.tool_search_has_candidates = has_deferred || has_pending_mcp;
        }
        ctx
    }
}

#[cfg(test)]
#[path = "tool_context.test.rs"]
mod tests;
