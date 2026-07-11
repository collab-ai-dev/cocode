use std::sync::Arc;

use anyhow::Result;
use coco_types::SessionId;

use super::{SessionRuntime, SessionRuntimeBuildOpts};

/// Cheap cloneable capability for a live session runtime.
///
/// The runtime stays private: callers operate through focused capabilities so
/// selecting a session and acting on it remain one explicit boundary.
#[derive(Clone)]
pub struct SessionHandle {
    session_id: SessionId,
    runtime: Arc<SessionRuntime>,
    callback_requirements: Arc<std::sync::OnceLock<coco_types::SessionCallbackRequirements>>,
}

impl SessionHandle {
    // Focused capability forwarding. Keeping these as explicit inherent
    // methods preserves the opaque runtime boundary while allowing callers to
    // request only the session service they need.
    pub fn runtime_config(&self) -> &Arc<coco_config::RuntimeConfig> {
        self.runtime.runtime_config()
    }

    pub fn tools(&self) -> &Arc<coco_tool_runtime::ToolRegistry> {
        self.runtime.tools()
    }

    pub fn app_state(&self) -> &Arc<tokio::sync::RwLock<coco_types::ToolAppState>> {
        self.runtime.app_state()
    }

    pub fn history(&self) -> &Arc<tokio::sync::Mutex<coco_messages::MessageHistory>> {
        self.runtime.history()
    }

    pub fn original_cwd(&self) -> &std::path::PathBuf {
        self.runtime.original_cwd()
    }

    pub fn project_root(&self) -> &std::path::PathBuf {
        self.runtime.project_root()
    }

    pub fn current_cwd(&self) -> &Arc<tokio::sync::RwLock<std::path::PathBuf>> {
        self.runtime.current_cwd()
    }

    pub fn config_home(&self) -> &std::path::PathBuf {
        self.runtime.config_home()
    }

    pub fn session_manager(&self) -> &Arc<coco_session::SessionManager> {
        self.runtime.session_manager()
    }

    pub fn project_services(&self) -> &Arc<coco_app_runtime::ProjectServices> {
        self.runtime.project_services()
    }

    pub fn process_runtime(&self) -> &Arc<coco_app_runtime::ProcessRuntime> {
        self.runtime.process_runtime()
    }

    pub fn file_read_state(&self) -> &Arc<tokio::sync::RwLock<coco_context::FileReadState>> {
        self.runtime.file_read_state()
    }

    pub fn file_history(
        &self,
    ) -> Option<&Arc<tokio::sync::RwLock<coco_context::FileHistoryState>>> {
        self.runtime.file_history()
    }

    pub fn hook_registry(&self) -> Arc<coco_hooks::HookRegistry> {
        self.runtime.hook_registry()
    }

    pub fn skill_manager(&self) -> Arc<coco_skills::SkillManager> {
        self.runtime.skill_manager()
    }

    pub fn model_runtimes(&self) -> Arc<coco_inference::ModelRuntimeRegistry> {
        self.runtime.model_runtimes()
    }

    pub fn sandbox_state(&self) -> Option<Arc<coco_sandbox::SandboxState>> {
        self.runtime.sandbox_state()
    }

    pub fn memory_runtime(&self) -> Option<&Arc<coco_memory::MemoryRuntime>> {
        self.runtime.memory_runtime()
    }

    pub fn command_queue(&self) -> &coco_query::CommandQueue {
        self.runtime.command_queue()
    }

    pub fn schedule_store(&self) -> coco_tool_runtime::ScheduleStoreRef {
        self.runtime.schedule_store()
    }

    pub fn side_query(&self) -> coco_tool_runtime::SideQueryHandle {
        self.runtime.side_query()
    }

    pub fn persist_session(&self) -> bool {
        self.runtime.persist_session()
    }

    pub fn runtime_publisher(&self) -> Option<Arc<coco_config::RuntimePublisher>> {
        self.runtime.runtime_publisher()
    }

    pub fn shutdown_child_token(&self) -> tokio_util::sync::CancellationToken {
        self.runtime.shutdown_child_token()
    }

    pub fn loop_sentinel_state(
        &self,
    ) -> &Arc<tokio::sync::Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>> {
        self.runtime.loop_sentinel_state()
    }

    pub fn usage_accounting(&self) -> coco_query::usage_accounting::UsageAccounting {
        self.runtime.usage_accounting()
    }

    pub fn skill_bash_cell(
        &self,
    ) -> Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>> {
        self.runtime.skill_bash_cell()
    }

    pub fn agent_catalog_handle(
        &self,
    ) -> Arc<tokio::sync::RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>> {
        self.runtime.agent_catalog_handle()
    }

    pub fn registered_tools(&self) -> Vec<Arc<dyn coco_tool_runtime::DynTool>> {
        self.runtime.registered_tools()
    }

    pub async fn current_typed_session_id(&self) -> SessionId {
        self.runtime.current_typed_session_id().await
    }

    pub async fn current_engine_config(&self) -> coco_query::QueryEngineConfig {
        self.runtime.current_engine_config().await
    }

    pub async fn current_task_runtime(&self) -> Option<Arc<crate::task_runtime::TaskRuntime>> {
        self.runtime.current_task_runtime().await
    }

    pub async fn current_task_list(&self) -> Option<coco_tool_runtime::TaskListHandleRef> {
        self.runtime.current_task_list().await
    }

    pub async fn current_team_task_list_router(
        &self,
    ) -> Option<coco_tool_runtime::TeamTaskListRouterRef> {
        self.runtime.current_team_task_list_router().await
    }

    pub async fn current_agent_handle(&self) -> Option<coco_tool_runtime::AgentHandleRef> {
        self.runtime.current_agent_handle().await
    }

    pub async fn current_agent_transcript_store(
        &self,
    ) -> Option<coco_tool_runtime::AgentTranscriptStoreRef> {
        self.runtime.current_agent_transcript_store().await
    }

    pub async fn current_fork_dispatcher(
        &self,
    ) -> Option<coco_query::forked_agent::ForkDispatcherRef> {
        self.runtime.current_fork_dispatcher().await
    }

    pub async fn current_mcp_handle(&self) -> Option<coco_tool_runtime::McpHandleRef> {
        self.runtime.current_mcp_handle().await
    }

    pub async fn current_command_registry(&self) -> Arc<coco_commands::CommandRegistry> {
        self.runtime.current_command_registry().await
    }

    pub async fn current_agent_catalog(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.runtime.current_agent_catalog().await
    }

    pub async fn agent_catalog_snapshot(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.runtime.agent_catalog_snapshot().await
    }

    pub async fn session_usage_snapshot(&self) -> coco_types::SessionUsageSnapshot {
        self.runtime.session_usage_snapshot().await
    }

    pub async fn last_cache_safe_params(&self) -> Option<coco_types::CacheSafeParams> {
        self.runtime.last_cache_safe_params().await
    }

    pub async fn fallback_cache_safe_params(&self) -> coco_types::CacheSafeParams {
        self.runtime.fallback_cache_safe_params().await
    }

    pub async fn build_engine(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> coco_query::QueryEngine {
        self.runtime.build_engine(cancel).await
    }

    pub async fn build_engine_from_config(
        &self,
        config: coco_query::QueryEngineConfig,
        cancel: tokio_util::sync::CancellationToken,
        app_state_override: Option<Arc<tokio::sync::RwLock<coco_types::ToolAppState>>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_engine_from_config(config, cancel, app_state_override)
            .await
    }

    pub(crate) async fn build_fork_engine_from_config(
        &self,
        config: coco_query::QueryEngineConfig,
        cancel: tokio_util::sync::CancellationToken,
        app_state_override: Option<Arc<tokio::sync::RwLock<coco_types::ToolAppState>>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_fork_engine_from_config(config, cancel, app_state_override)
            .await
    }

    pub(crate) async fn build_engine_from_config_with_registries(
        &self,
        config: coco_query::QueryEngineConfig,
        cancel: tokio_util::sync::CancellationToken,
        tools: Arc<coco_tool_runtime::ToolRegistry>,
        hooks: Option<Arc<coco_hooks::HookRegistry>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_engine_from_config_with_registries(config, cancel, tools, hooks)
            .await
    }

    pub async fn analyze_main_context(
        &self,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        self.runtime.analyze_main_context().await
    }

    pub async fn attach_agent_handle(&self, handle: coco_tool_runtime::AgentHandleRef) {
        self.runtime.attach_agent_handle(handle).await;
    }

    pub async fn attach_skill_handle(&self, handle: coco_tool_runtime::SkillHandleRef) {
        self.runtime.attach_skill_handle(handle).await;
    }

    pub async fn attach_fork_dispatcher(
        &self,
        dispatcher: coco_query::forked_agent::ForkDispatcherRef,
    ) {
        self.runtime.attach_fork_dispatcher(dispatcher).await;
    }

    pub async fn attach_hook_agent_runner(&self, runner: coco_query::hook_llm::HookAgentRunnerRef) {
        self.runtime.attach_hook_agent_runner(runner).await;
    }

    pub async fn attach_task_runtime(&self, runtime: Arc<crate::task_runtime::TaskRuntime>) {
        self.runtime.attach_task_runtime(runtime).await;
    }

    pub async fn attach_task_list(&self, handle: coco_tool_runtime::TaskListHandleRef) {
        self.runtime.attach_task_list(handle).await;
    }

    pub async fn attach_team_task_list_router(
        &self,
        router: coco_tool_runtime::TeamTaskListRouterRef,
    ) {
        self.runtime.attach_team_task_list_router(router).await;
    }

    pub async fn attach_agent_transcript_store(
        &self,
        store: coco_tool_runtime::AgentTranscriptStoreRef,
    ) {
        self.runtime.attach_agent_transcript_store(store).await;
    }

    pub async fn attach_mcp_handle(&self, handle: coco_tool_runtime::McpHandleRef) {
        self.runtime.attach_mcp_handle(handle).await;
    }

    pub async fn attach_mcp_manager(
        &self,
        manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    ) {
        self.runtime.attach_mcp_manager(manager).await;
    }

    pub async fn attach_lsp_handle(&self, handle: coco_tool_runtime::LspHandleRef) {
        self.runtime.attach_lsp_handle(handle).await;
    }

    pub async fn interrupt_agent_current_work(&self, agent_id: &str) -> Result<bool, String> {
        self.runtime.interrupt_agent_current_work(agent_id).await
    }

    pub async fn apply_permission_updates_everywhere(
        &self,
        updates: &[coco_types::PermissionUpdate],
    ) {
        self.runtime
            .apply_permission_updates_everywhere(updates)
            .await;
    }

    pub async fn apply_role_override(
        &self,
        role: coco_types::ModelRole,
        role_override: super::RoleOverride,
    ) -> anyhow::Result<()> {
        self.runtime.apply_role_override(role, role_override).await
    }

    pub async fn resolve_role(&self, role: coco_types::ModelRole) -> Option<super::RoleOverride> {
        self.runtime.resolve_role(role).await
    }

    pub fn apply_session_env_updates(
        &self,
        env: std::collections::HashMap<String, String>,
    ) -> (i32, i32) {
        self.runtime.apply_session_env_updates(env)
    }

    pub async fn explain_permission_risk(
        &self,
        params: coco_permissions::ExplainerParams<'_>,
    ) -> Option<coco_types::PermissionExplanation> {
        self.runtime.explain_permission_risk(params).await
    }

    pub async fn persist_goal_metadata(&self, goal: Option<coco_session::GoalMetadata>) {
        self.runtime.persist_goal_metadata(goal).await;
    }

    pub async fn persist_local_transcript_messages(&self, messages: &[coco_messages::Message]) {
        self.runtime
            .persist_local_transcript_messages(messages)
            .await;
    }

    pub fn update_session_registry_name(&self, name: &str) {
        self.runtime.update_session_registry_name(name);
    }

    pub async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        self.runtime.seed_transcript_dedup(uuids).await;
    }

    pub async fn seed_tool_result_replacement_state(
        &self,
        messages: &[coco_messages::Message],
        session_id: &SessionId,
        agent_id: Option<&str>,
    ) {
        self.runtime
            .seed_tool_result_replacement_state(messages, session_id, agent_id)
            .await;
    }

    pub async fn update_engine_config<F>(&self, update: F)
    where
        F: FnOnce(&mut coco_query::QueryEngineConfig),
    {
        self.runtime.update_engine_config(update).await;
    }

    pub async fn reload_plugins(&self, cwd: &std::path::Path) -> usize {
        self.runtime.reload_plugins(cwd).await
    }

    pub async fn reload_agent_catalog(&self) {
        self.runtime.reload_agent_catalog().await;
    }

    pub async fn reload_lsp_servers(&self) {
        self.runtime.reload_lsp_servers().await;
    }

    pub async fn reload_plugin_mcp_servers(&self) -> usize {
        self.runtime.reload_plugin_mcp_servers().await
    }

    pub fn mcp_reconnect_key(&self) -> u64 {
        self.runtime.mcp_reconnect_key()
    }

    pub async fn reload_hooks(&self) -> anyhow::Result<usize> {
        self.runtime.reload_hooks().await
    }

    pub async fn set_sdk_supplied_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        self.runtime.set_sdk_supplied_agents(agents).await;
    }

    pub async fn fire_session_start_hooks(&self, source: &str) {
        self.runtime.fire_session_start_hooks(source).await;
    }

    pub async fn fire_user_prompt_submit_hooks(
        &self,
        prompt: &str,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        self.runtime.fire_user_prompt_submit_hooks(prompt).await
    }

    pub async fn fire_notification_hooks(
        &self,
        notification_type: &str,
        message: &str,
        title: Option<&str>,
    ) {
        self.runtime
            .fire_notification_hooks(notification_type, message, title)
            .await;
    }

    pub async fn run_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        self.runtime
            .run_config_change_hooks(source, file_path)
            .await
    }

    pub async fn fire_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) {
        self.runtime
            .fire_config_change_hooks(source, file_path)
            .await;
    }

    pub async fn status_report(&self) -> String {
        self.runtime.status_report().await
    }

    pub fn resolve_model_selection(
        &self,
        value: &str,
    ) -> Option<coco_types::ProviderModelSelection> {
        self.runtime.resolve_model_selection(value)
    }

    pub fn live_permission_rules(
        &self,
    ) -> Arc<tokio::sync::RwLock<Vec<coco_types::PermissionRule>>> {
        self.runtime.live_permission_rules()
    }

    pub fn attachment_emitter(&self) -> coco_messages::AttachmentEmitter {
        self.runtime.attachment_emitter()
    }

    pub fn auto_title_enabled(&self) -> bool {
        self.runtime.auto_title_enabled()
    }

    pub fn fast_model_spec(&self) -> Option<&coco_types::ModelSpec> {
        self.runtime.fast_model_spec()
    }

    pub fn subscribe_config_changes(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<coco_config_reload::ConfigChange>> {
        self.runtime.subscribe_config_changes()
    }

    pub fn subscribe_config_reload_errors(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<coco_config_reload::ConfigReloadError>> {
        self.runtime.subscribe_config_reload_errors()
    }

    pub async fn install_side_query_event_tx(
        &self,
        event_tx: tokio::sync::mpsc::Sender<coco_query::CoreEvent>,
    ) {
        self.runtime.install_side_query_event_tx(event_tx).await;
    }

    pub async fn flush_session_usage_snapshot(&self) {
        self.runtime.flush_session_usage_snapshot().await;
    }

    pub async fn seed_todo_list_snapshot(&self, key: String, items: Vec<coco_types::TodoRecord>) {
        self.runtime.seed_todo_list_snapshot(key, items).await;
    }

    pub async fn seed_pre_clear_rewind_messages(
        &self,
        messages: Option<Vec<Arc<coco_messages::Message>>>,
    ) {
        self.runtime.seed_pre_clear_rewind_messages(messages).await;
    }

    pub async fn prepare_for_clear_replacement(&self) -> Option<Vec<Arc<coco_messages::Message>>> {
        self.runtime.prepare_for_clear_replacement().await
    }

    pub async fn pre_clear_rewind_messages(&self) -> Option<Vec<Arc<coco_messages::Message>>> {
        self.runtime.pre_clear_rewind_messages().await
    }

    pub async fn restore_pre_clear_rewind_prefix(
        &self,
        message_id: &str,
    ) -> Option<(i32, i32, Vec<coco_messages::Message>)> {
        self.runtime
            .restore_pre_clear_rewind_prefix(message_id)
            .await
    }

    pub async fn invoke_skill_fork(&self, name: &str, args: &str) -> Result<String, String> {
        self.runtime.invoke_skill_fork(name, args).await
    }

    pub async fn reload_plugins_with(
        &self,
        cwd: &std::path::Path,
        runtime_config: &coco_config::RuntimeConfig,
    ) -> usize {
        self.runtime.reload_plugins_with(cwd, runtime_config).await
    }

    pub async fn fire_setup_hooks(&self, trigger: coco_hooks::orchestration::SetupTrigger) {
        self.runtime.fire_setup_hooks(trigger).await;
    }

    pub(crate) fn start_active_turn(
        &self,
        build: impl FnOnce(
            coco_types::TurnId,
            tokio_util::sync::CancellationToken,
        ) -> super::ActiveTurnHandles,
    ) -> Result<coco_types::TurnId, ()> {
        self.runtime.turn_coordinator.start(&self.session_id, build)
    }

    pub(crate) fn active_turn_cancel_token(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.runtime.turn_coordinator.cancel_token()
    }

    pub(crate) fn has_active_turn(&self) -> bool {
        self.active_turn_cancel_token().is_some()
    }

    pub(crate) fn clear_active_turn(&self) -> bool {
        self.runtime.turn_coordinator.clear()
    }

    pub(crate) fn take_active_turn(&self) -> Option<super::ActiveTurnHandles> {
        self.runtime.turn_coordinator.take()
    }

    async fn drain_active_turn(&self, timeout: std::time::Duration) {
        let Some(mut active) = self.take_active_turn() else {
            return;
        };
        active.cancel_token.cancel();
        if tokio::time::timeout(timeout, &mut active.turn_task)
            .await
            .is_err()
        {
            active.turn_task.abort();
            let _ = active.turn_task.await;
        }
        if tokio::time::timeout(timeout, &mut active.forwarder_task)
            .await
            .is_err()
        {
            active.forwarder_task.abort();
            let _ = active.forwarder_task.await;
        }
    }

    pub async fn mcp_manager(
        &self,
    ) -> Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>> {
        self.runtime.current_mcp_manager().await
    }

    pub(crate) async fn record_mcp_registration_report(
        &self,
        server_name: &str,
        report: coco_tools::RegisterMcpToolsReport,
    ) {
        let status = super::McpRegistrationStatus {
            tool_count: report.registered.len() as i32,
            skipped_tools: report
                .skipped
                .into_iter()
                .map(|skipped| coco_types::McpSkippedToolStatus {
                    tool_name: skipped.tool_name,
                    error: skipped.error.to_string(),
                })
                .collect(),
            tombstoned_tools: report
                .tombstones
                .into_iter()
                .map(|tool_id| tool_id.to_string())
                .collect(),
        };
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .write()
            .await
            .insert(server_name.to_string(), status);
    }

    pub(crate) async fn mcp_registration_status(
        &self,
        server_name: &str,
    ) -> Option<super::McpRegistrationStatus> {
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .read()
            .await
            .get(server_name)
            .cloned()
    }

    pub(crate) async fn clear_mcp_registration_status(&self, server_name: &str) {
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .write()
            .await
            .remove(server_name);
    }

    pub async fn install_reload_supervisor(&self, handle: tokio::task::JoinHandle<()>) {
        let mut slot = self
            .runtime
            .integration_resources
            .reload_supervisor()
            .lock()
            .await;
        if let Some(previous) = slot.replace(handle) {
            previous.abort();
            let _ = previous.await;
        }
    }

    pub async fn stop_reload_supervisor(&self) {
        let handle = self
            .runtime
            .integration_resources
            .reload_supervisor()
            .lock()
            .await
            .take();
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
    }

    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        let session_id = runtime.current_typed_session_id_snapshot();
        Self {
            session_id,
            runtime,
            callback_requirements: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Self> {
        let runtime = SessionRuntime::build(opts).await?;
        Ok(Self::new(runtime))
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn install_callback_requirements(
        &self,
        requirements: coco_types::SessionCallbackRequirements,
    ) {
        let _ = self.callback_requirements.set(requirements);
    }

    pub fn callback_requirements(&self) -> coco_types::SessionCallbackRequirements {
        self.callback_requirements
            .get()
            .cloned()
            .unwrap_or_default()
    }

    /// Fire `SessionEnd` hooks and request runtime-scoped task shutdown only
    /// when this handle still owns the expected session id.
    ///
    /// Returns the runtime's current session id when the handle is stale.
    pub async fn close_if_current_session(
        &self,
        expected_session_id: &SessionId,
        reason: coco_hooks::orchestration::ExitReason,
        turn_drain_timeout: std::time::Duration,
    ) -> Option<SessionId> {
        let current_session_id = self.runtime.current_typed_session_id().await;
        if current_session_id != *expected_session_id {
            return Some(current_session_id);
        }

        self.drain_active_turn(turn_drain_timeout).await;
        self.stop_reload_supervisor().await;
        self.runtime.fire_session_end_hooks(reason).await;
        self.runtime.shutdown_signal().cancel();
        None
    }

    pub fn orchestration_ctx_factory(
        &self,
    ) -> Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync> {
        self.runtime.orchestration_ctx_factory()
    }
}
