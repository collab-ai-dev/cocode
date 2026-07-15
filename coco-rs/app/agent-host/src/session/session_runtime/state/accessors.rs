use super::*;

impl SessionRuntime {
    /// Session-scoped attachment emitter for producers outside the
    /// per-turn engine (TUI slash commands, swarm forwarders, ...).
    /// Each `emit()` enqueues a typed `AttachmentMessage` (typically
    /// silent-* variants) onto the session channel. The engine drains
    /// at the head of each outer-loop turn via
    /// [`coco_query::QueryEngine::drain_attachment_inbox`] so producers
    /// don't need access to `MessageHistory`.
    pub fn attachment_emitter(&self) -> coco_messages::AttachmentEmitter {
        self.command_resources.attachment_emitter()
    }
    /// The tool registry shared by every engine instance.
    /// Callers that need to register or deregister tools at runtime (e.g.
    /// the client-hosted MCP lifecycle handlers) use this to mutate the registry
    /// via its interior-mutability API.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        self.execution.tools()
    }
    /// Session-scoped sandbox state. Cheap-clone via `Arc`; consumers
    /// (fork dispatch, AppServer adapters) inherit the same instance so
    /// `SandboxState::update_config` hot-reloads propagate everywhere.
    pub fn sandbox_state(&self) -> Option<Arc<coco_sandbox::SandboxState>> {
        self.sandbox_resources.sandbox_state.clone()
    }
    /// Snapshot the current session id as a checked typed identity.
    pub async fn current_typed_session_id(&self) -> SessionId {
        self.engine_config_resources.session_id().clone()
    }
    /// Synchronous mirror of the current session id.
    ///
    /// This is used only to create cheap handle snapshots. Async runtime paths
    /// should prefer [`Self::current_typed_session_id`] while the fused runtime
    /// still exists.
    pub fn current_typed_session_id_snapshot(&self) -> SessionId {
        self.engine_config_resources.session_id().clone()
    }
    /// Whether this run persists session artifacts (transcript / usage /
    /// file-history / subagent transcripts). False under
    /// `--no-session-persistence`.
    pub fn persist_session(&self) -> bool {
        self.persistence.persist_session()
    }
    pub fn session_manager(&self) -> &Arc<coco_session::SessionManager> {
        self.persistence.session_manager()
    }
    pub fn project_paths(&self) -> &Arc<coco_paths::ProjectPaths> {
        self.persistence.project_paths()
    }
    pub fn transcript_store(&self) -> &Arc<dyn coco_session::SessionStore> {
        self.persistence.transcript_store()
    }
    /// The session's first-class goal aggregate (§10.2).
    pub fn goal_runtime(&self) -> &Arc<coco_goal_runtime::GoalRuntimeHandle> {
        self.persistence.goal_runtime()
    }

    /// The session-scoped runtime-owned evidence store (§10.2 #9), shared across
    /// per-turn goal handles and the goal driver's coordinator.
    pub fn goal_evidence(&self) -> &Arc<dyn coco_goal_runtime::EvidenceStore> {
        self.persistence.goal_evidence()
    }

    /// The goal continuation driver's cold-edge signal (§10.3).
    pub fn goal_driver_edge(&self) -> &Arc<tokio::sync::Notify> {
        self.persistence.goal_driver_edge()
    }
    pub fn fast_model_spec(&self) -> Option<&coco_types::ModelSpec> {
        self.title_resources.fast_model_spec()
    }
    pub fn auto_title_enabled(&self) -> bool {
        self.title_resources.auto_title_enabled()
    }
    pub fn original_cwd(&self) -> &std::path::PathBuf {
        self.workspace_resources.original_cwd()
    }
    pub fn project_root(&self) -> &std::path::PathBuf {
        self.workspace_resources.project_root()
    }
    pub fn current_cwd(&self) -> &Arc<RwLock<std::path::PathBuf>> {
        self.workspace_resources.current_cwd()
    }
    pub fn file_read_state(&self) -> &Arc<RwLock<coco_context::FileReadState>> {
        self.engine_state_resources.file_read_state()
    }
    pub fn file_history(&self) -> Option<&Arc<RwLock<coco_context::FileHistoryState>>> {
        self.engine_state_resources.file_history()
    }
    pub fn app_state(&self) -> &Arc<RwLock<coco_types::ToolAppState>> {
        self.engine_state_resources.app_state()
    }
    pub fn loop_sentinel_state(
        &self,
    ) -> &Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>> {
        self.engine_state_resources.loop_sentinel_state()
    }
    pub fn config_home(&self) -> &std::path::PathBuf {
        self.config_resources.config_home()
    }
    pub fn runtime_config(&self) -> &Arc<coco_config::RuntimeConfig> {
        self.config_resources.runtime_config()
    }
    pub fn process_runtime(&self) -> &Arc<coco_app_runtime::ProcessRuntime> {
        self.project_resources.process_runtime()
    }
    pub fn project_services(&self) -> &Arc<coco_app_runtime::ProjectServices> {
        self.project_resources.project_services()
    }
    /// Borrow the optional `MemoryRuntime`. `None` when
    /// `Feature::AutoMemory` is off. Callers (e.g. the slash dispatcher's
    /// `/dream` and `/summary` triggers) clone the inner `Arc`.
    pub fn memory_runtime(&self) -> Option<&Arc<coco_memory::MemoryRuntime>> {
        self.memory_resources.memory_runtime.as_ref()
    }
    /// The production swarm `AgentHandle` once `attach_agent_handle` has
    /// late-bound it (the eager `swarm_agent_handle` is a no-op until then).
    /// `None` before attach / in non-swarm sessions. Used by the leader
    /// inbox poller to resolve the active team via `active_team_name`.
    pub async fn current_agent_handle(&self) -> Option<AgentHandleRef> {
        self.handle_resources.agent_handle.read().await.clone()
    }
    /// Public accessor for the hook registry. Same `Arc` as the one
    /// installed on every per-turn engine; safe to clone.
    pub fn hook_registry(&self) -> Arc<coco_hooks::HookRegistry> {
        self.hook_resources.registry()
    }
    /// Public accessor for the session-scoped [`coco_skills::SkillManager`].
    /// Same `Arc` that backed the command-registry build and the
    /// reminder pipeline - safe to clone (cheap ref-count bump).
    /// Used by binary-entry wiring (e.g. `mcp_handle_adapter`) that
    /// sits outside the crate's `pub (crate)` field-access scope.
    pub fn skill_manager(&self) -> Arc<coco_skills::SkillManager> {
        Arc::clone(self.catalog_resources.skill_manager())
    }
    pub fn command_registry_slot(
        &self,
    ) -> &Arc<tokio::sync::RwLock<Arc<coco_commands::CommandRegistry>>> {
        self.catalog_resources.command_registry()
    }
    /// Session-scoped command queue handle. Producers outside the
    /// per-turn engine - the TUI bridge in the TUI driver (user typing
    /// while busy), future task-completion / coordinator / hook
    /// forwarders - call `enqueue` on this handle to inject mid-turn
    /// steering messages. Returned by reference; callers `.clone()` if
    /// they need an owned `Arc`-backed handle.
    /// Teammate messages and task notifications use the same queue
    /// with `QueueOrigin::Coordinator` / `QueueOrigin::TaskNotification`.
    pub fn command_queue(&self) -> &CommandQueue {
        self.command_resources.command_queue()
    }
    /// The session's schedule store (cron tasks + triggers). Shared with the
    /// cron tick driver ([`crate::cron_tick`]) so it reads/writes the same
    /// tasks the `Cron*` tools persist.
    pub fn schedule_store(&self) -> coco_tool_runtime::ScheduleStoreRef {
        self.turn_resources.schedule_store()
    }
}
