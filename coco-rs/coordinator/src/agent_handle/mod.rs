//! `AgentHandle` implementation bridging the tool layer → coordinator
//! orchestration.
//!
//! Dispatches to `spawnMultiAgent` / `runAgent` / `forkSubagent` based on
//! input parameters, implementing [`coco_tool_runtime::AgentHandle`] atop
//! the coordinator's runner + mailbox + team-file modules.
//!
//! Module layout (split from a single 854-LoC file):
//! - `mod.rs` (this file) — struct, accessors, setters, AgentHandle trait
//!   impl, teammate spawn.
//! - `spawn.rs` — sync + background subagent dispatch + worktree
//!   isolation + `AgentQueryConfig` construction.
//! - `handoff.rs` — 2-stage handoff safety classifier and post-spawn
//!   AgentSummary.
//! - `resume.rs` — background-spawn resume from JSONL transcript + sidecar
//!   metadata.

mod handoff;
mod resume;
mod spawn;
mod teammate_engine;

pub use teammate_engine::TeammateExecutionAdapter;
pub use teammate_engine::into_execution_engine;

use std::sync::Arc;

use arc_swap::ArcSwap;
use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;
use coco_tool_runtime::PlanApprovalMessage;
use coco_tool_runtime::PlanApprovalResponse;
use coco_tool_runtime::TeammateTaskRegistration;
use tokio::sync::RwLock;

use coco_types::TaskStatus;
use coco_types::TaskType;

use crate::constants::TEAM_LEAD_NAME;
use crate::identity::get_agent_id;
use crate::identity::get_agent_name;
use crate::identity::get_team_name;
use crate::mailbox::TeammateMessage;
use crate::mailbox::write_to_mailbox;
use crate::roster_store::CommitMemberRequest;
use crate::roster_store::DeleteTeamRequest;
use crate::roster_store::SpawnMemberRequest;
use crate::roster_store::TeamRosterStore;
use crate::runner::InProcessAgentRunner;
use crate::runner::SpawnConfig;
use crate::teammate::resolve_teammate_model;
use crate::types::TeamManager;

/// AgentHandle implementation backed by the swarm infrastructure.
///
/// The bridge between the tool layer (AgentTool) and the state layer
/// (swarm modules). Routes spawn requests to the appropriate backend
/// (in-process, tmux, iTerm2) and manages agent lifecycle.
pub struct SwarmAgentHandle {
    runner: Arc<InProcessAgentRunner>,
    backend_registry: Option<Arc<crate::pane::BackendRegistry>>,
    team_manager: Arc<RwLock<Option<TeamManager>>>,
    roster_store: TeamRosterStore,
    /// Drives the LLM loop for sync subagents. `None` ⇒ sync spawn fails
    /// fast with a "no engine configured" error rather than silently
    /// succeeding with placeholder output. Install via
    /// [`Self::set_execution_engine`] at session bootstrap.
    execution_engine: Option<coco_tool_runtime::AgentQueryEngineRef>,
    /// `None` ⇒ worktree-isolation requests fail fast. The CLI resolves
    /// the canonical git root at bootstrap and installs this so subagents
    /// spawned with worktree isolation always land in
    /// `.coco/worktrees/agent-<slug>` under the main repo.
    worktree_manager: Option<Arc<crate::worktree::AgentWorktreeManager>>,
    /// Drives the 2-stage handoff safety classifier. `None` ⇒ classifier
    /// is a no-op (fail-open, matches TS).
    side_query: Option<coco_tool_runtime::SideQueryHandle>,
    /// Required running-task registry (`AgentTaskRegistry`). Same `Arc`
    /// as the engine's `TaskHandle` slot so AgentTool spawns registered
    /// here are addressable through `Task*` tools the model invokes later.
    /// There is no Swarm-side LocalAgent fallback store.
    task_registry: coco_tool_runtime::AgentTaskRegistryRef,
    /// Durable task-list handle shared with the leader engine. In-process
    /// teammates poll this after mailbox messages so unclaimed team tasks
    /// become work prompts without going through a separate mirror.
    task_list: Option<coco_tool_runtime::TaskListHandleRef>,
    task_list_router: Option<coco_tool_runtime::TeamTaskListRouterRef>,
    /// Per-agent transcript persistence. When installed, bg AgentTool
    /// spawns write `AgentSpawnMetadata` at registration and the full
    /// message history to `agent-<id>.jsonl` on completion. `resume_agent`
    /// reads both files to rehydrate a stopped spawn. `None` ⇒ resume is
    /// unsupported in this session.
    transcript_store: Option<coco_tool_runtime::AgentTranscriptStoreRef>,
    cwd: String,
    /// Atomic snapshot of resolved providers + role mappings. Wrapped
    /// in [`ArcSwap`] so [`Self::set_runtime_config`] can hot-swap
    /// without `&mut self` — production holds `Arc<SwarmAgentHandle>`,
    /// so `Arc::get_mut` is unreachable and the prior `&mut self`
    /// setter was silently broken in the SettingsWatcher path.
    runtime_config: Arc<ArcSwap<coco_config::RuntimeConfig>>,
    /// Drives the in-process teammate runner-loop after `spawn_teammate`
    /// registers a teammate. `None` ⇒ teammate spawns succeed at
    /// registration but never execute LLM turns (the prior behaviour
    /// before Gap C was fixed). Wire via
    /// [`Self::set_teammate_execution_engine`] at session bootstrap.
    teammate_engine: Option<Arc<dyn crate::runner_loop::AgentExecutionEngine>>,
    /// Auto-compact threshold (token count) used by the teammate
    /// runner-loop. Defaults to 100k; override via
    /// [`Self::set_teammate_auto_compact_threshold`] so the leader's
    /// resolved `CompactConfig` flows through to teammates.
    teammate_auto_compact_threshold: i64,
    /// Base system prompt (the main agent's full system prompt) used
    /// as the teammate's base in `build_teammate_system_prompt`. The
    /// runner-loop appends `TEAMMATE_PROMPT_ADDENDUM` to whatever this
    /// holds. `None` ⇒ teammates see only the addendum (fixed by
    /// [`Self::set_teammate_base_system_prompt`]). The CLI wires this by
    /// passing `runtime.current_engine_config().system_prompt` here at
    /// bootstrap.
    teammate_base_system_prompt: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Hook registry used to fire `SubagentStart` / `SubagentStop`
    /// around subagent execution. `None` ⇒ hooks don't fire (the
    /// pre-fix behaviour). `SubagentStart.additionalContexts` are
    /// injected as hook_additional_context attachments into the child's
    /// first user message; the stop hook fires at completion / error /
    /// cancel.
    hook_registry: Option<Arc<coco_hooks::HookRegistry>>,
    /// Skill handle used to preload skill bodies declared in agent
    /// frontmatter (`skills: [foo, bar]`). At spawn time the handle's
    /// `read_skill_body(name)` is called for each entry and the
    /// concatenated bodies are prepended to the child's first user
    /// message. `None` ⇒ frontmatter skills are silently ignored
    /// (logged at debug).
    skill_handle: Option<coco_tool_runtime::SkillHandleRef>,
    /// MCP handle used to register agent-specific MCP servers
    /// declared in frontmatter (`mcpServers: [github, {slack: {...}}]`).
    /// At spawn: inline entries get `add_dynamic_server`'d; at stop:
    /// they're `remove_dynamic_server`'d. String-ref entries are
    /// pre-registered on the handle (no spawn-time mutation). `None`
    /// ⇒ inline entries are silently dropped (logged at debug).
    mcp_handle: Option<coco_tool_runtime::McpHandleRef>,
    /// Per-agent dynamic MCP server tracking. Populated when an
    /// inline server gets stood up at spawn time; consulted at
    /// SubagentStop to teardown only the agent's own servers
    /// (string-ref entries point at parent-shared connections that
    /// must NOT be torn down).
    dynamic_mcp_servers: Arc<tokio::sync::RwLock<std::collections::HashMap<String, Vec<String>>>>,
    /// Builder closure invoked at spawn time when the target subagent
    /// is `coco-guide` to populate the dynamic context block (custom
    /// skills / agents / MCP servers / plugin commands / settings.json).
    ///
    /// `None` ⇒ no dynamic block is appended (static base prompt only).
    /// The CLI bootstrap wires this from `CommandRegistry` +
    /// `AgentCatalogSnapshot` + `McpHandle::connected_servers` +
    /// settings.json. Kept as a builder closure rather than 4
    /// separate Arc fields so future additions to the block don't
    /// proliferate SwarmAgentHandle fields.
    coco_guide_context_builder: Option<CocoGuideContextBuilder>,
}

/// Closure type for [`SwarmAgentHandle::coco_guide_context_builder`].
/// Returns an owned snapshot — callers can read short-lived registry
/// state inside without lifetime entanglement at the use site.
pub type CocoGuideContextBuilder =
    Arc<dyn Fn() -> coco_subagent::CocoGuideDynamicContext + Send + Sync>;

impl SwarmAgentHandle {
    pub fn new(
        runner: Arc<InProcessAgentRunner>,
        team_manager: Arc<RwLock<Option<TeamManager>>>,
        cwd: String,
        runtime_config: Arc<coco_config::RuntimeConfig>,
        task_registry: coco_tool_runtime::AgentTaskRegistryRef,
    ) -> Self {
        let roster_store = TeamRosterStore::new(team_manager.clone());
        Self {
            runner,
            backend_registry: None,
            team_manager,
            roster_store,
            execution_engine: None,
            worktree_manager: None,
            side_query: None,
            task_registry,
            task_list: None,
            task_list_router: None,
            transcript_store: None,
            cwd,
            runtime_config: Arc::new(ArcSwap::from(runtime_config)),
            teammate_engine: None,
            teammate_auto_compact_threshold: 100_000,
            teammate_base_system_prompt: Arc::new(tokio::sync::RwLock::new(None)),
            hook_registry: None,
            skill_handle: None,
            mcp_handle: None,
            dynamic_mcp_servers: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            coco_guide_context_builder: None,
        }
    }

    /// Install the coco-guide dynamic context builder.
    /// Without this hook, spawned `coco-guide` agents see only the
    /// static base prompt — losing visibility into the user's custom
    /// skills / agents / MCP servers / settings, matching the
    /// pre-Phase-1 coco-rs behavior. CLI bootstrap typically wires
    /// this from the CommandRegistry + active AgentCatalogSnapshot +
    /// the McpHandle's connected-servers list + settings.json.
    pub fn set_coco_guide_context_builder(&mut self, builder: CocoGuideContextBuilder) {
        self.coco_guide_context_builder = Some(builder);
    }

    pub(crate) fn coco_guide_context_builder(&self) -> Option<&CocoGuideContextBuilder> {
        self.coco_guide_context_builder.as_ref()
    }

    pub fn set_backend_registry(&mut self, registry: Arc<crate::pane::BackendRegistry>) {
        self.backend_registry = Some(registry);
    }

    pub fn set_task_list(&mut self, handle: coco_tool_runtime::TaskListHandleRef) {
        self.task_list = Some(handle);
    }

    pub fn set_team_task_list_router(&mut self, router: coco_tool_runtime::TeamTaskListRouterRef) {
        self.task_list_router = Some(router);
    }

    /// Install the MCP handle used for per-agent dynamic server
    /// registration (inline `mcpServers: [{name: config}]`).
    pub fn set_mcp_handle(&mut self, handle: coco_tool_runtime::McpHandleRef) {
        self.mcp_handle = Some(handle);
    }

    pub(crate) fn mcp_handle(&self) -> Option<&coco_tool_runtime::McpHandleRef> {
        self.mcp_handle.as_ref()
    }

    pub(crate) fn dynamic_mcp_servers(
        &self,
    ) -> &Arc<tokio::sync::RwLock<std::collections::HashMap<String, Vec<String>>>> {
        &self.dynamic_mcp_servers
    }

    /// Install a skill handle for frontmatter-skill preload at spawn
    /// time. The handle's `read_skill_body(name)` resolves each name
    /// declared in `definition.skills` to its prompt body, which then
    /// gets prepended to the child's first user message.
    pub fn set_skill_handle(&mut self, handle: coco_tool_runtime::SkillHandleRef) {
        self.skill_handle = Some(handle);
    }

    pub(crate) fn skill_handle(&self) -> Option<&coco_tool_runtime::SkillHandleRef> {
        self.skill_handle.as_ref()
    }

    /// Install the hook registry used to fire `SubagentStart` /
    /// `SubagentStop` around subagent execution. Without this,
    /// user-defined hooks for those events silently never run for
    /// spawned subagents.
    pub fn set_hook_registry(&mut self, registry: Arc<coco_hooks::HookRegistry>) {
        self.hook_registry = Some(registry);
    }

    pub(crate) fn hook_registry(&self) -> Option<&Arc<coco_hooks::HookRegistry>> {
        self.hook_registry.as_ref()
    }

    /// Set the base system prompt teammates receive (composed with
    /// `TEAMMATE_PROMPT_ADDENDUM` by the runner-loop). The CLI wires
    /// this from the leader's resolved system prompt so teammates
    /// inherit CLAUDE.md + env-context + memory blocks the same way
    /// the leader sees them.
    pub async fn set_teammate_base_system_prompt(&self, prompt: String) {
        let mut slot = self.teammate_base_system_prompt.write().await;
        *slot = Some(prompt);
    }

    /// Install the engine that drives in-process teammate runner-loops.
    /// Without this, `spawn_teammate` registers the teammate but no
    /// LLM turn ever runs (Gap C). Production wiring lives in
    /// `app/cli/agent_handle_factory::install_agent_team`.
    pub fn set_teammate_execution_engine(
        &mut self,
        engine: Arc<dyn crate::runner_loop::AgentExecutionEngine>,
    ) {
        self.teammate_engine = Some(engine);
    }

    /// Override the teammate runner-loop's auto-compact threshold. The
    /// CLI wires this from `runtime_config.compact` so user-tuned
    /// thresholds flow through to teammates.
    pub fn set_teammate_auto_compact_threshold(&mut self, threshold: i64) {
        self.teammate_auto_compact_threshold = threshold;
    }

    /// Interrupt an in-process teammate's active turn without killing
    /// the teammate lifecycle. Escape in a teammate transcript aborts
    /// the current iteration; the teammate returns to idle and can
    /// receive more prompts.
    pub async fn interrupt_teammate_current_work(&self, agent_id: &str) -> Result<bool, String> {
        self.task_registry
            .interrupt_teammate_current_work(agent_id)
            .await
    }

    /// Hot-swap the resolved `RuntimeConfig`. Atomic via `ArcSwap` —
    /// no `&mut self` required, so the SettingsWatcher can update the
    /// snapshot even when the handle is shared via `Arc`.
    pub fn set_runtime_config(&self, runtime_config: Arc<coco_config::RuntimeConfig>) {
        self.runtime_config.store(runtime_config);
    }

    /// Snapshot the current `RuntimeConfig` for read-only consumers.
    pub(crate) fn runtime_config(&self) -> Arc<coco_config::RuntimeConfig> {
        self.runtime_config.load_full()
    }

    /// Resolve a role's primary `model_id`, mirroring `ModelRoles::get`'s
    /// Main-fallback so the value matches whatever the engine factory's
    /// `resolve_agent_runtime_source` ultimately runs for that role.
    /// Returns `Err` when neither the role nor Main is configured.
    pub(crate) fn current_model_id_for_role(
        &self,
        role: coco_types::ModelRole,
    ) -> Result<String, &'static str> {
        self.runtime_config()
            .model_roles
            .get(role)
            .map(|spec| spec.model_id.clone())
            .ok_or("requested role (and Main fallback) not configured in RuntimeConfig.model_roles")
    }

    /// Resolve the Main role's `model_id`. Returns `Err` when no
    /// Main role is configured — the previous silent-empty-string
    /// fallback hid the misconfiguration from the caller and surfaced
    /// it later as a confusing provider error.
    pub(crate) fn current_main_model_id(&self) -> Result<String, &'static str> {
        self.current_model_id_for_role(coco_types::ModelRole::Main)
    }

    /// Resolve the concrete bare `model_id` to render in a child's
    /// `<env>` block, derived from the SAME
    /// [`coco_types::LlmModelSelection`] that drives routing so the
    /// displayed model matches the one the engine factory resolves.
    /// `Role`/`InheritMain` resolve through [`Self::current_model_id_for_role`]
    /// (Main-fallback), `Explicit*` read the structured (already-bare)
    /// `primary.model_id`. Catalog-canonicalization for display happens
    /// once downstream in `coco_context::get_environment_info`. Empty
    /// when nothing resolves — the env block then omits the "powered by"
    /// line.
    pub(crate) fn model_id_for_env(&self, selection: &coco_types::LlmModelSelection) -> String {
        match selection {
            coco_types::LlmModelSelection::Explicit { primary }
            | coco_types::LlmModelSelection::ExplicitWithFallbackRole { primary, .. } => {
                primary.model_id.clone()
            }
            coco_types::LlmModelSelection::Role { role } => {
                self.current_model_id_for_role(*role).unwrap_or_default()
            }
            coco_types::LlmModelSelection::InheritMain => {
                self.current_main_model_id().unwrap_or_default()
            }
        }
    }

    pub(crate) fn execution_engine(&self) -> Option<coco_tool_runtime::AgentQueryEngineRef> {
        self.execution_engine.clone()
    }

    pub(crate) fn worktree_manager(&self) -> Option<&Arc<crate::worktree::AgentWorktreeManager>> {
        self.worktree_manager.as_ref()
    }

    pub(crate) fn side_query(&self) -> Option<&coco_tool_runtime::SideQueryHandle> {
        self.side_query.as_ref()
    }

    pub(crate) fn task_registry(&self) -> &coco_tool_runtime::AgentTaskRegistryRef {
        &self.task_registry
    }

    /// Required for `resume_agent`; optional for fresh spawns (the bg path
    /// silently skips persistence when absent).
    pub fn set_transcript_store(&mut self, store: coco_tool_runtime::AgentTranscriptStoreRef) {
        self.transcript_store = Some(store);
    }

    pub(crate) fn transcript_store(&self) -> Option<&coco_tool_runtime::AgentTranscriptStoreRef> {
        self.transcript_store.as_ref()
    }

    pub fn set_execution_engine(&mut self, engine: coco_tool_runtime::AgentQueryEngineRef) {
        self.execution_engine = Some(engine);
    }

    /// Install an [`AgentWorktreeManager`](crate::worktree::AgentWorktreeManager)
    /// so subagents spawned with `isolation: "worktree"` get a real git
    /// worktree + cwd_override + cleanup-on-success.
    pub fn set_worktree_manager(&mut self, manager: Arc<crate::worktree::AgentWorktreeManager>) {
        self.worktree_manager = Some(manager);
    }

    /// Without a side-query bridge the classifier silently passes
    /// everything through (fail-open).
    pub fn set_side_query(&mut self, handle: coco_tool_runtime::SideQueryHandle) {
        self.side_query = Some(handle);
    }

    fn is_teammate_spawn(request: &AgentSpawnRequest) -> bool {
        request.name.is_some() && request.team_name.is_some()
    }

    async fn spawn_teammate(
        &self,
        request: &AgentSpawnRequest,
    ) -> Result<AgentSpawnResponse, String> {
        let requested_name = request
            .name
            .as_deref()
            .ok_or("name required for teammate")?;
        let team_name = request
            .team_name
            .as_deref()
            .ok_or("team_name required for teammate")?;

        let runtime_config = self.runtime_config();
        let main_model_id = self
            .current_main_model_id()
            .map_err(|e| format!("teammate spawn: {e}"))?;
        // Per-request `model` slot is gone — read from definition only.
        // `resolve_teammate_model` accepts `Option<&str>`; passing `None`
        // makes it use the team config's default model or the
        // role-resolved spec instead.
        let definition_model = request.definition.as_ref().and_then(|d| d.model.as_deref());
        let resolved_model = resolve_teammate_model(
            definition_model,
            &main_model_id,
            &runtime_config.agent_teams,
            request.subagent_type.as_deref(),
            |role| {
                runtime_config
                    .model_roles
                    .get(role)
                    .map(|spec| spec.model_id.clone())
            },
        );

        // Prefer per-spawn `initial_prompt` override; otherwise fall back to
        // the leader's full system prompt so the teammate sees the same
        // CLAUDE.md + env-context + memory blocks the leader does. The
        // runner-loop then composes this with `TEAMMATE_PROMPT_ADDENDUM`.
        // Pre-fix: teammates ran with ONLY the addendum (the leader's
        // system prompt was discarded).
        // `initial_prompt` flows from `AgentDefinition.initial_prompt`
        // (frontmatter). Top-level `request.initial_prompt` was a dead
        // slot and is gone.
        let teammate_system_prompt = match request
            .definition
            .as_ref()
            .and_then(|d| d.initial_prompt.clone())
        {
            Some(p) => Some(p),
            None => self.teammate_base_system_prompt.read().await.clone(),
        };

        // Persistent round-robin assignment so the same teammate gets
        // the same color across spawns within a session. The agent_id
        // namespacing keeps the assignment scoped per teammate identity.
        let reservation = self
            .roster_store
            .reserve_member(SpawnMemberRequest {
                desired_name: requested_name.to_string(),
                team_name: team_name.to_string(),
                agent_type: request.subagent_type.clone(),
                model: Some(resolved_model.model.clone()),
                prompt: request.prompt.clone(),
                color: None,
                plan_mode_required: request.mode == Some(coco_types::PermissionMode::Plan),
                cwd: request
                    .cwd
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| self.cwd.clone()),
                worktree_path: None,
                mode: request.mode,
            })
            .await?;
        let name = reservation.name.as_str();

        let agent_id_for_color = reservation.agent_id.clone();
        let color = crate::pane::layout::assign_teammate_color(&agent_id_for_color);
        self.roster_store
            .set_member_color(team_name, &reservation.agent_id, color.as_str().to_string())
            .await?;

        let config = SpawnConfig {
            name: name.to_string(),
            team_name: team_name.to_string(),
            prompt: request.prompt.clone(),
            color: Some(color.as_str().to_string()),
            plan_mode_required: request.mode == Some(coco_types::PermissionMode::Plan),
            model: Some(resolved_model.model.clone()),
            working_dir: request.cwd.as_ref().map(|p| p.display().to_string()),
            system_prompt: teammate_system_prompt,
            allowed_tools: Vec::new(),
            allow_permission_prompts: true,
            // Static effort lives on `AgentDefinition.effort`. Read it
            // through here (was: blank pass-through of unset
            // `request.effort`). See `agent_handle.rs` comment on
            // `AgentSpawnRequest` for why per-spawn override slot was
            // removed.
            // All static knobs read through `request.definition` — the
            // previously-dead top-level slots are gone. See
            // `agent_handle.rs` `AgentSpawnRequest` field comment.
            effort: request.definition.as_ref().and_then(|d| d.effort),
            use_exact_tools: request
                .definition
                .as_ref()
                .map(|d| d.use_exact_tools)
                .unwrap_or(false),
            isolation: coco_types::AgentIsolation::None,
            memory_scope: None,
            mcp_servers: request
                .definition
                .as_ref()
                .map(|d| {
                    d.mcp_servers
                        .iter()
                        .filter_map(|spec| spec.name().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            disallowed_tools: request
                .definition
                .as_ref()
                .map(|d| d.disallowed_tools.clone())
                .unwrap_or_default(),
            max_turns: request
                .constraints
                .as_ref()
                .and_then(|c| c.max_turns)
                .or_else(|| request.definition.as_ref().and_then(|d| d.max_turns)),
        };

        let mut launched_executor: Option<Arc<dyn crate::pane::TeammateExecutor>> = None;
        let selected_backend = if let Some(registry) = self.backend_registry.as_ref() {
            let executor = registry
                .select_teammate_executor(
                    runtime_config.agent_teams.teammate_mode,
                    request.is_non_interactive,
                )
                .await?;
            launched_executor = Some(executor.clone());
            let spawn = executor
                .spawn(crate::pane::TeammateSpawnConfig {
                    name: name.to_string(),
                    team_name: team_name.to_string(),
                    color: Some(color),
                    plan_mode_required: config.plan_mode_required,
                    prompt: request.prompt.clone(),
                    cwd: request
                        .cwd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| self.cwd.clone()),
                    model: config.model.clone(),
                    system_prompt: config.system_prompt.clone(),
                    system_prompt_mode: crate::pane::SystemPromptMode::Default,
                    worktree_path: None,
                    parent_session_id: request.session_id.clone(),
                    permissions: config.allowed_tools.clone(),
                    allow_permission_prompts: config.allow_permission_prompts,
                    // All static knobs read from `request.definition` —
                    // see `agent_handle.rs` `AgentSpawnRequest` field
                    // comment for why the top-level slots are gone.
                    effort: request.definition.as_ref().and_then(|d| d.effort),
                    use_exact_tools: request
                        .definition
                        .as_ref()
                        .map(|d| d.use_exact_tools)
                        .unwrap_or(false),
                    mcp_servers: request
                        .definition
                        .as_ref()
                        .map(|d| {
                            d.mcp_servers
                                .iter()
                                .filter_map(|spec| spec.name().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    disallowed_tools: request
                        .definition
                        .as_ref()
                        .map(|d| d.disallowed_tools.clone())
                        .unwrap_or_default(),
                    max_turns: request
                        .constraints
                        .as_ref()
                        .and_then(|c| c.max_turns)
                        .or_else(|| request.definition.as_ref().and_then(|d| d.max_turns)),
                })
                .await;
            (executor.backend_type(), spawn)
        } else {
            let spawn = self.runner.register_agent(config.clone()).await;
            (
                crate::types::BackendType::InProcess,
                crate::pane::TeammateSpawnResult {
                    success: spawn.success,
                    agent_id: spawn.agent_id,
                    error: spawn.error,
                    task_id: None,
                    pane_id: None,
                },
            )
        };
        let (spawn_backend_type, spawn_result) = selected_backend;

        if !spawn_result.success {
            let _ = self
                .roster_store
                .rollback_member(team_name, &reservation.agent_id)
                .await;
            return Ok(AgentSpawnResponse {
                status: AgentSpawnStatus::Failed,
                agent_id: Some(spawn_result.agent_id),
                result: None,
                error: spawn_result.error,
                total_tool_use_count: 0,
                total_tokens: 0,
                duration_ms: 0,
                worktree_path: None,
                worktree_branch: None,
                output_file: None,
                prompt: None,
                ..Default::default()
            });
        }

        let task_cancel = tokio_util::sync::CancellationToken::new();
        let teammate_task_id = self
            .task_registry
            .register_teammate_task(TeammateTaskRegistration::new(
                name.to_string(),
                team_name.to_string(),
                spawn_backend_type,
                spawn_result.pane_id.clone(),
                request.prompt.clone(),
                task_cancel.clone(),
            ))
            .await;
        let _team_member = match self
            .roster_store
            .commit_member(CommitMemberRequest {
                team_name: team_name.to_string(),
                agent_id: reservation.agent_id.clone(),
                backend_type: spawn_backend_type,
                pane_id: spawn_result.pane_id.clone(),
                // The parent session id is not the teammate session id.
                // Pane/in-process children fill this on reconnect/report.
                session_id: None,
            })
            .await
        {
            Ok(member) => member,
            Err(e) => {
                if let Some(executor) = launched_executor.as_ref() {
                    let _ = executor.kill(&spawn_result.agent_id).await;
                } else {
                    let _ = self.runner.cancel_agent(&spawn_result.agent_id).await;
                }
                self.task_registry
                    .complete_teammate_task(
                        &spawn_result.agent_id,
                        TaskStatus::Failed,
                        None,
                        Some(e.clone()),
                    )
                    .await;
                let _ = self
                    .roster_store
                    .rollback_member(team_name, &reservation.agent_id)
                    .await;
                return Ok(AgentSpawnResponse {
                    status: AgentSpawnStatus::Failed,
                    agent_id: Some(spawn_result.agent_id),
                    result: None,
                    error: Some(e),
                    total_tool_use_count: 0,
                    total_tokens: 0,
                    duration_ms: 0,
                    worktree_path: None,
                    worktree_branch: None,
                    output_file: None,
                    prompt: None,
                    ..Default::default()
                });
            }
        };

        // ── Gap C fix — actually start the teammate's LLM loop ──
        //
        // Pre-fix: production code stopped at `register_agent`; the
        // teammate sat in the runner's `agents` map forever and never
        // ran a single turn.
        //
        // Now: when a teammate execution engine is installed, build the
        // runner config + task-state mirror, kick off the runner_loop
        // in a detached task, and wire its JoinHandle into the runner
        // so `wait_for_completion` / `cancel_agent` work.
        if spawn_backend_type == crate::types::BackendType::InProcess
            && let Some(engine) = self.teammate_engine.clone()
        {
            let cancelled = self
                .runner
                .get_context(&spawn_result.agent_id)
                .await
                .map(|ctx| ctx.cancelled)
                .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
            let cancel_flag = cancelled.clone();
            let task_cancel_for_flag = task_cancel.clone();
            let registry_for_cancel = self.task_registry.clone();
            let agent_id_for_cancel = spawn_result.agent_id.clone();
            tokio::spawn(async move {
                task_cancel_for_flag.cancelled().await;
                cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                registry_for_cancel
                    .complete_teammate_task(
                        &agent_id_for_cancel,
                        TaskStatus::Killed,
                        None,
                        Some("killed".to_string()),
                    )
                    .await;
            });

            let identity = crate::types::TeammateIdentity {
                agent_id: spawn_result.agent_id.clone(),
                agent_name: name.to_string(),
                team_name: team_name.to_string(),
                color: config
                    .color
                    .as_deref()
                    .and_then(|c| c.parse::<coco_types::AgentColorName>().ok()),
                plan_mode_required: config.plan_mode_required,
            };

            // Wire TeammateIdle hook context. SwarmAgentHandle owns
            // the registry; we synthesize the orchestration context
            // here using the same helper subagent spawning uses
            // (`spawn::hook_ctx_for_subagent`).
            let teammate_orchestration_ctx = self.hook_registry().map(|_| {
                crate::agent_handle::spawn::hook_ctx_for_subagent(
                    &self.cwd,
                    Some(&spawn_result.agent_id),
                    request.subagent_type.as_deref(),
                )
            });
            let runner_config = crate::runner_loop::InProcessRunnerConfig {
                identity,
                task_id: teammate_task_id.clone(),
                session_id: request.session_id.clone(),
                prompt: request.prompt.clone(),
                model: config.model.clone(),
                system_prompt: config.system_prompt.clone(),
                system_prompt_mode: crate::pane::SystemPromptMode::Default,
                allowed_tools: config.allowed_tools.clone(),
                allow_permission_prompts: config.allow_permission_prompts,
                max_turns: config.max_turns,
                cancelled,
                auto_compact_threshold: self.teammate_auto_compact_threshold,
                // Intentional invariant (A7b): teammates NEVER inherit the
                // leader's bypass-permissions capability — they always run
                // through the normal permission gate. Hardcoded `false` rather
                // than threading the parent's flag, so a future change to grant
                // teammates bypass is an explicit, reviewable edit.
                bypass_permissions_available: false,
                features: request.features.clone(),
                tool_overrides: request.tool_overrides.clone(),
                parent_tool_filter: request.parent_tool_filter.clone(),
                active_shell_tool: request.active_shell_tool,
                // All static knobs read from `request.definition` —
                // see `agent_handle.rs` `AgentSpawnRequest` field
                // comment for why the top-level slots are gone.
                effort: request.definition.as_ref().and_then(|d| d.effort),
                use_exact_tools: request
                    .definition
                    .as_ref()
                    .map(|d| d.use_exact_tools)
                    .unwrap_or(false),
                mcp_servers: request
                    .definition
                    .as_ref()
                    .map(|d| {
                        d.mcp_servers
                            .iter()
                            .filter_map(|spec| spec.name().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                disallowed_tools: request
                    .definition
                    .as_ref()
                    .map(|d| d.disallowed_tools.clone())
                    .unwrap_or_default(),
                model_role: resolved_model.model_role,
                model_selection: resolved_model.model_selection.clone(),
                task_list: self.task_list.clone(),
                task_registry: Some(self.task_registry.clone()),
                roster_store: Some(self.roster_store.clone()),
                plan_mode_required: config.plan_mode_required,
                hooks: self.hook_registry().cloned(),
                orchestration_ctx: teammate_orchestration_ctx,
            };

            let registry = self.task_registry.clone();
            let agent_id = spawn_result.agent_id.clone();
            let join = tokio::spawn(async move {
                let result =
                    crate::runner_loop::run_in_process_teammate(runner_config, engine.as_ref())
                        .await;
                registry
                    .complete_teammate_task(
                        &agent_id,
                        if result.success {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed
                        },
                        result.output.clone(),
                        result.error.clone(),
                    )
                    .await;
                result
            });
            self.runner.start_agent(&spawn_result.agent_id, join).await;
        } else if spawn_backend_type == crate::types::BackendType::InProcess {
            tracing::warn!(
                agent_id = %spawn_result.agent_id,
                "teammate registered without execution engine — no LLM turns will run; \
                 install via SwarmAgentHandle::set_teammate_execution_engine at session bootstrap"
            );
        } else if let Some(executor) = launched_executor {
            let registry = self.task_registry.clone();
            let agent_id = spawn_result.agent_id.clone();
            let task_cancel_for_pane = task_cancel.clone();
            tokio::spawn(async move {
                task_cancel_for_pane.cancelled().await;
                let _ = executor.kill(&agent_id).await;
                registry
                    .complete_teammate_task(
                        &agent_id,
                        TaskStatus::Killed,
                        None,
                        Some("killed".to_string()),
                    )
                    .await;
            });
        }

        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::TeammateSpawned,
            agent_id: Some(spawn_result.agent_id),
            result: None,
            error: None,
            total_tool_use_count: 0,
            total_tokens: 0,
            duration_ms: 0,
            worktree_path: None,
            worktree_branch: None,
            output_file: None,
            prompt: None,
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl AgentHandle for SwarmAgentHandle {
    async fn spawn_agent(&self, request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        if Self::is_teammate_spawn(&request) {
            self.spawn_teammate(&request).await
        } else {
            self.spawn_subagent(&request).await
        }
    }

    async fn resume_agent(
        &self,
        agent_id: &str,
        prompt: &str,
        session_id: &str,
    ) -> Result<AgentSpawnResponse, String> {
        // Inherent `resume_agent` lives in `resume.rs`; delegate so the
        // trait surface is satisfied without duplicating the body.
        SwarmAgentHandle::resume_agent(self, agent_id, prompt.to_string(), session_id.to_string())
            .await
    }

    async fn send_message(&self, to: &str, content: &str) -> Result<String, String> {
        let team_name = {
            let tm = self.team_manager.read().await;
            tm.as_ref()
                .map(|m| m.team_name().to_string())
                .or_else(get_team_name)
                .ok_or_else(|| "No active team — cannot send message".to_string())?
        };

        let from = get_agent_name().unwrap_or_else(|| TEAM_LEAD_NAME.to_string());

        if to == "*" {
            let members = self.roster_store.broadcast_recipients(&from).await;

            let mut sent = Vec::new();
            for recipient in &members {
                let message = TeammateMessage {
                    from: from.clone(),
                    text: content.to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    read: false,
                    color: crate::pane::layout::get_teammate_color(&from)
                        .map(|c| c.as_str().to_string()),
                    summary: None,
                };
                if write_to_mailbox(recipient, message, &team_name).is_ok() {
                    sent.push(recipient.clone());
                }
            }
            return Ok(format!(
                "Broadcast from '{from}' to {} recipients: {}",
                sent.len(),
                sent.join(", ")
            ));
        }

        let message = TeammateMessage {
            from: from.clone(),
            text: content.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            read: false,
            color: crate::pane::layout::get_teammate_color(&from).map(|c| c.as_str().to_string()),
            summary: None,
        };

        write_to_mailbox(to, message, &team_name)
            .map_err(|e| format!("Failed to send message to '{to}': {e}"))?;

        Ok(format!("Message sent from '{from}' to '{to}'"))
    }

    async fn create_team(
        &self,
        request: coco_tool_runtime::CreateTeamRequest,
    ) -> Result<coco_tool_runtime::CreateTeamResult, String> {
        let result = self.roster_store.create_team(request).await?;
        Ok(coco_tool_runtime::CreateTeamResult {
            team_name: result.team_name,
            lead_agent_id: result.lead_agent_id,
            task_list_id: result.task_list_id,
        })
    }

    async fn delete_team(&self) -> Result<String, String> {
        // When no team is active, return success with a "nothing to clean
        // up" message (idempotent). Pass the session task-list handle so
        // the roster store can fire a "tasks changed" notification on the
        // success path. At this point the route still points at the team
        // list; the notification reaches its subscribers before
        // `clear_team_task_list_route` restores the session list below.
        let result = self
            .roster_store
            .delete_team(DeleteTeamRequest, self.task_list.as_deref())
            .await?;
        if result.deleted
            && let Some(router) = &self.task_list_router
        {
            router
                .clear_team_task_list_route()
                .await
                .map_err(|e| format!("Failed to restore session task list: {e}"))?;
        }
        let Some(name) = result.team_name else {
            return Ok("No team name found, nothing to clean up".into());
        };

        // Implementation gap: `clearLeaderTeamName()` and the app-state reset
        // (`teamContext: undefined`, `inbox.messages: []`) live outside the
        // coordinator crate. The CLI / state owner observes deletion via
        // the returned message and performs those resets. Tracked in
        // `docs/coco-rs/agentteam-architecture.md` "delete_team parity".

        Ok(format!(
            "Cleaned up directories and worktrees for team \"{name}\""
        ))
    }

    async fn active_team_name(&self) -> Option<String> {
        self.roster_store.active_team_name().await
    }

    async fn query_agent_status(&self, agent_id: &str) -> Result<AgentSpawnResponse, String> {
        match AgentIdentity::classify(agent_id) {
            AgentIdentity::LocalAgent => self.query_local_agent_status(agent_id).await,
            AgentIdentity::TeamAgent => self.query_team_agent_status(agent_id).await,
        }
    }

    async fn get_agent_output(&self, agent_id: &str) -> Result<String, String> {
        match AgentIdentity::classify(agent_id) {
            AgentIdentity::LocalAgent => self.get_local_agent_output(agent_id).await,
            AgentIdentity::TeamAgent => self.get_team_agent_output(agent_id).await,
        }
    }

    async fn interrupt_agent_current_work(&self, agent_id: &str) -> Result<bool, String> {
        self.interrupt_teammate_current_work(agent_id).await
    }

    async fn request_shutdown(&self, target: &str, reason: Option<&str>) -> Result<String, String> {
        if target == "*" {
            return Err("shutdown_request cannot be broadcast — name a single teammate".into());
        }
        if target == TEAM_LEAD_NAME {
            return Err("cannot request the team lead to shut down".into());
        }
        let team_name = {
            let tm = self.team_manager.read().await;
            tm.as_ref()
                .map(|m| m.team_name().to_string())
                .or_else(get_team_name)
                .ok_or_else(|| "No active team — cannot request shutdown".to_string())?
        };
        let from = get_agent_name().unwrap_or_else(|| TEAM_LEAD_NAME.to_string());
        let request_id =
            crate::mailbox::send_shutdown_request(target, &team_name, &from, reason)
                .map_err(|e| format!("Failed to send shutdown request to '{target}': {e}"))?;
        Ok(format!(
            "Shutdown requested for '{target}' (request {request_id}). \
             Awaiting the teammate's approval."
        ))
    }

    async fn respond_to_shutdown(
        &self,
        request_id: &str,
        approve: bool,
        reason: Option<&str>,
    ) -> Result<String, String> {
        // Only teammates respond to shutdown; the leader never does.
        // Resolve this worker's own identity from the 3-tier resolver.
        let from = get_agent_name().ok_or_else(|| {
            "shutdown_response requires a teammate identity (no agent name resolved)".to_string()
        })?;
        let team_name = get_team_name().ok_or_else(|| {
            "shutdown_response requires an active team (no team resolved)".to_string()
        })?;
        let agent_id = get_agent_id().unwrap_or_else(|| format!("{from}@{team_name}"));

        // Read this worker's OWN pane coordinates from team.json so the
        // leader can kill the right pane. In-process teammates carry an
        // empty pane id + InProcess backend → the leader skips kill_pane
        // and the teammate exits via its runner-loop break.
        let (pane_id, backend_type) = self_pane_coords(&team_name, &agent_id);

        let text = if approve {
            crate::mailbox::create_shutdown_approved_message(
                request_id,
                &from,
                pane_id.as_deref(),
                backend_type,
            )
        } else {
            crate::mailbox::create_shutdown_rejected_message(
                request_id,
                &from,
                reason.unwrap_or("shutdown rejected by teammate"),
            )
        };
        let summary = if approve {
            "shutdown approved"
        } else {
            "shutdown rejected"
        };
        let message = TeammateMessage {
            from: from.clone(),
            text,
            timestamp: chrono::Utc::now().to_rfc3339(),
            read: false,
            color: crate::pane::layout::get_teammate_color(&from).map(|c| c.as_str().to_string()),
            summary: Some(summary.to_string()),
        };
        write_to_mailbox(TEAM_LEAD_NAME, message, &team_name)
            .map_err(|e| format!("Failed to send shutdown response to team lead: {e}"))?;

        // On approval, abort our OWN in-process runner loop (TS
        // `handleShutdownApproval` → `task.abortController.abort()`). This
        // tool runs inline inside the teammate's task-local scope, so
        // `signal_self_stop` reaches the runner's `config.cancelled` flag.
        // No-op (returns false) for cross-process teammates — those exit
        // when the leader's `teardown_teammate` kills their pane instead.
        if approve {
            crate::identity::signal_self_stop();
        }

        Ok(if approve {
            format!(
                "Shutdown approved. Confirmation sent to {TEAM_LEAD_NAME}; \
                 wrap up and exit."
            )
        } else {
            format!("Shutdown rejected. Notified {TEAM_LEAD_NAME} and continuing work.")
        })
    }

    async fn respond_to_plan_approval(
        &self,
        target: &str,
        request_id: &str,
        approve: bool,
        feedback: Option<&str>,
        permission_mode: coco_types::PermissionMode,
    ) -> Result<String, String> {
        if target == TEAM_LEAD_NAME {
            return Err("plan_approval_response must target the requesting teammate".into());
        }

        let from = get_agent_name().unwrap_or_else(|| TEAM_LEAD_NAME.to_string());
        if from != TEAM_LEAD_NAME {
            return Err("only the team lead can approve or reject teammate plans".into());
        }

        let team_name = {
            let tm = self.team_manager.read().await;
            tm.as_ref()
                .map(|m| m.team_name().to_string())
                .or_else(get_team_name)
                .ok_or_else(|| "No active team — cannot respond to plan approval".to_string())?
        };

        let inherited_mode = match permission_mode {
            coco_types::PermissionMode::Plan => coco_types::PermissionMode::Default,
            other => other,
        };
        let response = PlanApprovalMessage::PlanApprovalResponse(PlanApprovalResponse {
            request_id: request_id.to_string(),
            approved: approve,
            feedback: feedback.map(str::to_string),
            permission_mode: approve.then_some(inherited_mode),
        });
        let text = serde_json::to_string(&response)
            .map_err(|e| format!("Failed to serialize plan approval response: {e}"))?;
        let summary = if approve {
            "plan approved"
        } else {
            "plan rejected"
        };
        let message = TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text,
            timestamp: chrono::Utc::now().to_rfc3339(),
            read: false,
            color: crate::pane::layout::get_teammate_color(TEAM_LEAD_NAME)
                .map(|c| c.as_str().to_string()),
            summary: Some(summary.to_string()),
        };
        write_to_mailbox(target, message, &team_name)
            .map_err(|e| format!("Failed to send plan approval response to '{target}': {e}"))?;

        Ok(if approve {
            format!("Plan approved for '{target}' (request {request_id}).")
        } else {
            format!("Plan rejected for '{target}' (request {request_id}).")
        })
    }

    async fn teardown_teammate(
        &self,
        agent_id: &str,
        name: &str,
        pane_id: Option<&str>,
        backend_type: Option<crate::types::BackendType>,
    ) -> Result<(), String> {
        let Some(team_name) = self.roster_store.active_team_name().await else {
            return Err("no active team — cannot tear down teammate".into());
        };

        // 1. Kill the pane for pane-based teammates. In-process teammates
        //    have no pane id (and an InProcess backend) and exit via their
        //    own runner-loop break, so kill_pane is skipped.
        let is_in_process = backend_type == Some(crate::types::BackendType::InProcess);
        if let Some(pane) = pane_id.filter(|p| !p.is_empty())
            && !is_in_process
            && let Some(registry) = &self.backend_registry
            && let Some(backend) = registry.get_pane_backend().await
        {
            // Kill on the backend the teammate was actually created on. A
            // session hosts a single pane backend today so this normally
            // matches; guard defensively so a future mixed tmux/iTerm2 team
            // never kills the wrong server's pane.
            let registered = backend.backend_type();
            if backend_type.is_some_and(|bt| bt != registered) {
                tracing::warn!(agent_id, pane_id = pane, msg_backend = ?backend_type,
                    registered = registered.as_str(), "shutdown teardown: backend_type mismatch — skipping kill_pane");
            } else if let Err(e) = backend.kill_pane(&pane.to_string()).await {
                tracing::warn!(agent_id, pane_id = pane, error = %e,
                    "shutdown teardown: kill_pane failed (continuing)");
            }
        }

        // 2. Remove the teammate from the team file + live roster.
        if let Err(e) = self
            .roster_store
            .rollback_member(&team_name, agent_id)
            .await
        {
            tracing::warn!(agent_id, error = %e,
                "shutdown teardown: remove member failed (continuing)");
        }

        // 3. Unassign its in-flight tasks so peers can reclaim them, and
        //    notify the leader which tasks reopened so its model can
        //    reassign them. Pushes a `teammate_terminated` message built
        //    from the unassigned list; the poller re-injects it as a
        //    coordinator turn.
        if let Some(task_list) = &self.task_list {
            match task_list.unassign_teammate_tasks(agent_id, name).await {
                Ok(reopened) if !reopened.is_empty() => {
                    let list = reopened
                        .iter()
                        .map(|(id, subject)| format!("- {subject} ({id})"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = format!(
                        "Teammate '{name}' was terminated. {n} task(s) returned to the \
                         pool for reassignment:\n{list}",
                        n = reopened.len()
                    );
                    let message = TeammateMessage {
                        from: name.to_string(),
                        text,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        read: false,
                        color: None,
                        summary: Some(format!("{name} terminated")),
                    };
                    if let Err(e) = write_to_mailbox(TEAM_LEAD_NAME, message, &team_name) {
                        tracing::warn!(agent_id, error = %e,
                            "shutdown teardown: terminate notification failed (continuing)");
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(agent_id, error = %e,
                    "shutdown teardown: unassign tasks failed (continuing)"),
            }
        }

        // 4. Mark a PANE teammate's running task terminal. In-process
        //    teammates already get this from their runner wrapper
        //    (`complete_teammate_task` on loop exit); pane teammates have no
        //    such runner, so the row would otherwise linger `Running` forever.
        //    Idempotent — no-ops on an already-terminal row.
        if !is_in_process {
            self.task_registry
                .complete_teammate_task(agent_id, coco_types::TaskStatus::Completed, None, None)
                .await;
        }

        Ok(())
    }

    async fn set_teammate_mode(
        &self,
        name: &str,
        mode: coco_types::PermissionMode,
    ) -> Result<String, String> {
        let team_name = self
            .roster_store
            .active_team_name()
            .await
            .ok_or_else(|| "no active team — cannot set teammate mode".to_string())?;
        // 1. Persist to team.json + live roster.
        self.roster_store
            .set_member_mode(&team_name, name, mode)
            .await?;
        // 2. Notify the live teammate via a ModeSetRequest in its mailbox.
        let message = TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text: crate::mailbox::create_mode_set_request(mode, TEAM_LEAD_NAME),
            timestamp: chrono::Utc::now().to_rfc3339(),
            read: false,
            color: None,
            summary: Some("mode change".to_string()),
        };
        write_to_mailbox(name, message, &team_name)
            .map_err(|e| format!("Failed to notify teammate '{name}' of mode change: {e}"))?;
        Ok(format!("Set '{name}' permission mode to {mode:?}"))
    }

    async fn set_teammate_modes(
        &self,
        updates: Vec<(String, coco_types::PermissionMode)>,
    ) -> Result<String, String> {
        let team_name = self
            .roster_store
            .active_team_name()
            .await
            .ok_or_else(|| "no active team — cannot set teammate modes".to_string())?;
        // 1. ONE atomic team.json write for all changed members (TS
        //    `setMultipleMemberModes` — avoids the N-write race of looping
        //    `set_member_mode`).
        self.roster_store
            .set_member_modes(&team_name, &updates)
            .await?;
        // 2. Notify every targeted teammate via a `ModeSetRequest` so its live
        //    permission context updates. Mails ALL teammates in the batch,
        //    not only the ones whose stored mode changed.
        for (name, mode) in &updates {
            let message = TeammateMessage {
                from: TEAM_LEAD_NAME.to_string(),
                text: crate::mailbox::create_mode_set_request(*mode, TEAM_LEAD_NAME),
                timestamp: chrono::Utc::now().to_rfc3339(),
                read: false,
                color: None,
                summary: Some("mode change".to_string()),
            };
            if let Err(e) = write_to_mailbox(name, message, &team_name) {
                tracing::warn!(teammate = %name, error = %e,
                    "set_teammate_modes: mailbox notify failed (continuing)");
            }
        }
        Ok(format!(
            "Set permission mode for {} teammate(s)",
            updates.len()
        ))
    }
}

/// Read a worker's own `(pane_id, backend_type)` from its team.json
/// member entry. Returns `(None, None)` when the file or member is
/// missing. The pane id is normalised to `None` when empty (the
/// in-process case).
fn self_pane_coords(
    team_name: &str,
    agent_id: &str,
) -> (Option<String>, Option<crate::types::BackendType>) {
    match crate::team_file::read_team_file(team_name) {
        Ok(Some(tf)) => tf
            .members
            .into_iter()
            .find(|m| m.agent_id == agent_id)
            .map(|m| {
                let pane = (!m.tmux_pane_id.is_empty()).then_some(m.tmux_pane_id);
                (pane, m.backend_type)
            })
            .unwrap_or((None, None)),
        _ => (None, None),
    }
}

impl SwarmAgentHandle {
    async fn query_local_agent_status(&self, agent_id: &str) -> Result<AgentSpawnResponse, String> {
        let task = self
            .task_registry
            .task_state(agent_id)
            .await
            .ok_or_else(|| format!("Agent '{agent_id}' not found"))?;
        if task.task_type() != TaskType::BgAgent {
            return Err(format!("Agent '{agent_id}' not found"));
        }
        let status = match task.status {
            TaskStatus::Pending | TaskStatus::Running => AgentSpawnStatus::AsyncLaunched,
            TaskStatus::Completed => AgentSpawnStatus::Completed,
            TaskStatus::Failed | TaskStatus::Killed => AgentSpawnStatus::Failed,
        };
        Ok(AgentSpawnResponse {
            status,
            agent_id: Some(agent_id.to_string()),
            result: if task.status.is_terminal() {
                Some(self.task_registry.read_output(agent_id).await)
            } else {
                None
            },
            error: None,
            duration_ms: task
                .end_time
                .map(|end| end.saturating_sub(task.start_time))
                .unwrap_or_default(),
            output_file: task.output_file.map(std::path::PathBuf::from),
            ..Default::default()
        })
    }

    async fn query_team_agent_status(&self, agent_id: &str) -> Result<AgentSpawnResponse, String> {
        let member = self
            .team_member(agent_id)
            .await?
            .ok_or_else(|| format!("Agent '{agent_id}' not found"))?;
        let task = self
            .task_registry
            .teammate_task_state(&member.agent_id)
            .await
            .ok_or_else(|| {
                format!("Agent '{agent_id}' is not locally controllable in this process")
            })?;

        let status = match task.status {
            TaskStatus::Pending | TaskStatus::Running => AgentSpawnStatus::AsyncLaunched,
            TaskStatus::Completed => AgentSpawnStatus::Completed,
            TaskStatus::Failed | TaskStatus::Killed => AgentSpawnStatus::Failed,
        };

        Ok(AgentSpawnResponse {
            status,
            agent_id: Some(agent_id.to_string()),
            result: task
                .teammate_extras()
                .and_then(|extras| extras.result.clone().or_else(|| extras.error.clone())),
            error: None,
            total_tool_use_count: 0,
            total_tokens: 0,
            duration_ms: 0,
            worktree_path: None,
            worktree_branch: None,
            output_file: None,
            prompt: None,
            ..Default::default()
        })
    }

    async fn get_local_agent_output(&self, agent_id: &str) -> Result<String, String> {
        let task = self
            .task_registry
            .task_state(agent_id)
            .await
            .ok_or_else(|| format!("Agent '{agent_id}' not found"))?;
        if task.task_type() != TaskType::BgAgent {
            return Err(format!("Agent '{agent_id}' not found"));
        }
        Ok(self.task_registry.read_output(agent_id).await)
    }

    async fn get_team_agent_output(&self, agent_id: &str) -> Result<String, String> {
        let member = self
            .team_member(agent_id)
            .await?
            .ok_or_else(|| format!("Agent '{agent_id}' not found"))?;
        let task = self
            .task_registry
            .teammate_task_state(&member.agent_id)
            .await
            .ok_or_else(|| {
                format!("Agent '{agent_id}' is not locally controllable in this process")
            })?;
        let output = self.task_registry.read_output(&task.id).await;
        if output.is_empty() {
            return Err(format!("Agent '{agent_id}' has no output yet"));
        }
        Ok(output)
    }

    async fn team_member(
        &self,
        agent_id: &str,
    ) -> Result<Option<crate::types::TeamMember>, String> {
        let Some((name, team_name)) = agent_id.split_once('@') else {
            return Ok(None);
        };
        let team_file = crate::team_file::read_team_file(team_name)
            .map_err(|e| format!("Failed to read team '{team_name}': {e}"))?;
        Ok(team_file.and_then(|team| {
            team.members
                .into_iter()
                .find(|member| member.agent_id == agent_id || member.name == name)
        }))
    }
}

enum AgentIdentity {
    LocalAgent,
    TeamAgent,
}

impl AgentIdentity {
    fn classify(id: &str) -> Self {
        if id.contains('@') {
            Self::TeamAgent
        } else {
            Self::LocalAgent
        }
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
