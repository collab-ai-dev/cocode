use std::{sync::Arc, time::Instant};

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

pub struct QueuedCommandStatus {
    pub is_empty: bool,
    pub last_changed_at: Instant,
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

    pub async fn prompt_history_texts(&self, project: String) -> Vec<String> {
        self.runtime.prompt_history_texts(project).await
    }

    pub async fn persist_prompt_history_entry(
        &self,
        project: String,
        display: String,
    ) -> anyhow::Result<()> {
        self.runtime
            .persist_prompt_history_entry(project, display)
            .await
    }

    pub fn session_manager_handle(&self) -> Arc<coco_session::SessionManager> {
        Arc::clone(self.runtime.session_manager())
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

    pub fn file_history_enabled(&self) -> bool {
        self.runtime.file_history().is_some()
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

    pub async fn queued_command_status(&self) -> QueuedCommandStatus {
        let queue = self.runtime.command_queue();
        QueuedCommandStatus {
            is_empty: queue.is_empty().await,
            last_changed_at: queue.last_changed_at(),
        }
    }

    pub fn subscribe_command_queue_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.runtime.command_queue().subscribe_changes()
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

    pub async fn list_session_tasks(&self) -> Option<Vec<coco_types::TaskStateBase>> {
        self.runtime.list_session_tasks().await
    }

    pub async fn read_session_task_outputs(
        &self,
        task_id: &str,
    ) -> Result<coco_tool_runtime::TerminalOutputs, super::SessionTaskError> {
        self.runtime.read_session_task_outputs(task_id).await
    }

    pub async fn stop_session_task(&self, task_id: &str) -> Result<(), super::SessionTaskError> {
        self.runtime.stop_session_task(task_id).await
    }

    pub async fn background_all_session_tasks(&self) -> Vec<String> {
        self.runtime.background_all_session_tasks().await
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

    pub async fn live_session_summary_and_history(
        &self,
    ) -> (coco_types::SessionSummary, Vec<Arc<coco_messages::Message>>) {
        let config = self.current_engine_config().await;
        let history = self.history_messages().await;
        let usage = self.session_usage_snapshot().await;
        let timestamp = chrono::Utc::now().to_rfc3339();
        (
            coco_types::SessionSummary {
                session_id: self.session_id().clone(),
                model: config.model_id,
                cwd: self.original_cwd().to_string_lossy().into_owned(),
                created_at: timestamp.clone(),
                updated_at: Some(timestamp),
                title: None,
                message_count: history.len() as i32,
                total_tokens: usage
                    .totals
                    .input_tokens
                    .saturating_add(usage.totals.output_tokens),
            },
            history,
        )
    }

    pub async fn bypass_permissions_available(&self) -> bool {
        self.current_engine_config()
            .await
            .permission_mode_availability
            .bypass_permissions
    }

    pub async fn workspace_cwd(&self) -> std::path::PathBuf {
        self.current_engine_config().await.workspace_cwd()
    }

    pub async fn thinking_level(&self) -> Option<coco_types::ThinkingLevel> {
        self.current_engine_config().await.thinking_level
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

    pub async fn build_turn_engine(
        &self,
        request: super::SessionTurnEngineConfigRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> super::SessionTurnEngine {
        self.runtime.build_turn_engine(request, cancel).await
    }

    pub async fn run_manual_compact(
        &self,
        request: coco_query::ManualCompactRequest,
        event_tx: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        self.runtime
            .run_manual_compact(request, event_tx, cancel)
            .await;
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

    pub async fn apply_model_role_selection(
        &self,
        selection: super::SessionModelRoleSelection,
    ) -> anyhow::Result<super::SessionModelRoleChange> {
        self.runtime.apply_model_role_selection(selection).await
    }

    pub async fn resolve_role(&self, role: coco_types::ModelRole) -> Option<super::RoleOverride> {
        self.runtime.resolve_role(role).await
    }

    pub async fn model_role_change_snapshot(
        &self,
        role: coco_types::ModelRole,
        effort: Option<coco_types::ReasoningEffort>,
    ) -> Option<super::SessionModelRoleChange> {
        self.runtime.model_role_change_snapshot(role, effort).await
    }

    pub fn apply_session_env_updates(
        &self,
        env: std::collections::HashMap<String, String>,
    ) -> (i32, i32) {
        self.runtime.apply_session_env_updates(env)
    }

    pub fn session_env_snapshot(&self) -> Option<std::collections::HashMap<String, String>> {
        self.runtime.session_env_snapshot()
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

    pub async fn active_goal_snapshot(&self) -> Option<coco_types::ActiveGoal> {
        self.runtime.active_goal_snapshot().await
    }

    pub async fn restore_goal_from_history(
        &self,
        messages: &[Arc<coco_messages::Message>],
        trust_rejected: bool,
    ) -> Option<coco_types::ActiveGoal> {
        self.runtime
            .restore_goal_from_history(messages, trust_rejected)
            .await
    }

    pub async fn persist_local_transcript_messages(&self, messages: &[coco_messages::Message]) {
        self.runtime
            .persist_local_transcript_messages(messages)
            .await;
    }

    pub async fn append_messages_to_history(
        &self,
        messages: Vec<coco_messages::Message>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime.append_messages_to_history(messages).await
    }

    pub async fn append_messages_to_history_and_emit(
        &self,
        messages: Vec<coco_messages::Message>,
        event_tx: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime
            .append_messages_to_history_and_emit(messages, event_tx)
            .await
    }

    pub async fn history_messages(&self) -> Vec<Arc<coco_messages::Message>> {
        self.runtime.history_messages().await
    }

    pub async fn truncate_history_at_user_message(
        &self,
        message_id: &str,
    ) -> Result<super::SessionHistoryTruncateResult, usize> {
        self.runtime
            .truncate_history_at_user_message(message_id)
            .await
    }

    pub async fn append_arc_messages_to_history_and_snapshot(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime
            .append_arc_messages_to_history_and_snapshot(messages)
            .await
    }

    pub async fn replace_history_with_arc_messages(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) {
        self.runtime
            .replace_history_with_arc_messages(messages)
            .await;
    }

    pub async fn commit_engine_turn_history(&self, history: coco_messages::MessageHistory) {
        self.runtime.commit_engine_turn_history(history).await;
    }

    pub async fn commit_compacted_history(&self, history: coco_messages::MessageHistory) {
        self.runtime.commit_compacted_history(history).await;
    }

    pub async fn re_append_session_metadata(&self) {
        self.runtime.re_append_session_metadata().await;
    }

    pub async fn has_persisted_title(&self) -> bool {
        self.runtime.has_persisted_title().await
    }

    pub async fn persist_session_title(&self, name: String) -> anyhow::Result<()> {
        self.runtime.persist_session_title(name.clone()).await?;
        self.runtime.update_session_registry_name(&name);
        Ok(())
    }

    pub async fn title_generation_conversation_text(&self) -> String {
        self.runtime.title_generation_conversation_text().await
    }

    pub async fn list_persisted_session_summaries(
        &self,
    ) -> anyhow::Result<coco_types::SessionListResult> {
        self.runtime.list_persisted_session_summaries().await
    }

    pub async fn persist_session_mode(&self) {
        self.runtime.persist_session_mode().await;
    }

    pub fn reconcile_session_mode_on_resume(
        &self,
        stored_mode: Option<&str>,
    ) -> Option<&'static str> {
        self.runtime.reconcile_session_mode_on_resume(stored_mode)
    }

    pub async fn toggle_tag(&self, tag: String) -> anyhow::Result<(SessionId, bool)> {
        self.runtime.toggle_tag(tag).await
    }

    pub async fn rewind_files(
        &self,
        request: super::SessionFileRewindRequest,
    ) -> Result<super::SessionFileRewindResult, super::SessionFileRewindError> {
        self.runtime.rewind_files(request).await
    }

    pub async fn render_session_file_diff(
        &self,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        self.runtime.render_session_file_diff().await
    }

    pub async fn rewind_diff_stats(
        &self,
        message_id: &str,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.runtime.rewind_diff_stats(message_id).await
    }

    pub async fn rewind_diff_stats_between(
        &self,
        message_id: &str,
        next_message_id: Option<&str>,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.runtime
            .rewind_diff_stats_between(message_id, next_message_id)
            .await
    }

    pub async fn render_turn_file_diff(
        &self,
        message_id: &str,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        self.runtime.render_turn_file_diff(message_id).await
    }

    pub fn update_session_registry_name(&self, name: &str) {
        self.runtime.update_session_registry_name(name);
    }

    pub(crate) async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        self.runtime.seed_transcript_dedup(uuids).await;
    }

    pub(crate) async fn seed_tool_result_replacement_state(
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

    pub async fn set_model_id(&self, model_id: String) -> String {
        self.runtime.set_model_id(model_id).await
    }

    pub async fn set_thinking_level(&self, thinking_level: Option<coco_types::ThinkingLevel>) {
        self.runtime.set_thinking_level(thinking_level).await;
    }

    pub async fn set_fast_mode(&self, active: bool) {
        self.runtime.set_fast_mode(active).await;
    }

    pub async fn set_requires_structured_output(&self, active: bool) {
        self.runtime.set_requires_structured_output(active).await;
    }

    pub async fn install_structured_output_tool_if_requested(
        &self,
        raw_schema: Option<&str>,
    ) -> anyhow::Result<bool> {
        let Some(raw) = raw_schema else {
            return Ok(false);
        };
        let schema: serde_json::Value = serde_json::from_str(raw)
            .map_err(|error| anyhow::anyhow!("--json-schema is not valid JSON: {error}"))?;
        coco_tools::register_structured_output_tool(self.tools(), schema)
            .map_err(|error| anyhow::anyhow!("--json-schema rejected: {error}"))?;
        self.set_requires_structured_output(true).await;
        tracing::info!(
            target: "coco_agent_host::structured_output",
            "registered StructuredOutput tool from --json-schema"
        );
        Ok(true)
    }

    pub async fn set_skill_overrides(&self, skill_overrides: Arc<coco_config::SkillOverrideTiers>) {
        self.runtime.set_skill_overrides(skill_overrides).await;
    }

    pub async fn apply_session_start_config(&self, config: super::SessionStartRuntimeConfig) {
        self.runtime.apply_session_start_config(config).await;
    }

    pub async fn apply_turn_runtime_config(&self, config: super::SessionTurnRuntimeConfig) {
        self.runtime.apply_turn_runtime_config(config).await;
    }

    pub async fn set_live_permissions(&self, permissions: coco_types::LiveToolPermissionState) {
        self.runtime.set_live_permissions(permissions).await;
    }

    pub async fn reset_session_permission_rules(&self) -> (usize, usize) {
        self.runtime.reset_session_permission_rules().await
    }

    pub async fn set_permission_mode(
        &self,
        mode: coco_types::PermissionMode,
    ) -> super::PermissionModeChange {
        self.runtime.set_permission_mode(mode).await
    }

    pub async fn effective_permission_mode(&self) -> coco_types::PermissionMode {
        self.runtime.effective_permission_mode().await
    }

    pub async fn additional_working_dirs(&self) -> Vec<std::path::PathBuf> {
        self.runtime.additional_working_dirs().await
    }

    pub async fn refresh_live_permissions_for_turn(
        &self,
        refresh: super::SessionTurnPermissionRefresh,
    ) {
        self.runtime
            .refresh_live_permissions_for_turn(refresh)
            .await;
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

    pub async fn set_client_supplied_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        self.runtime.set_client_supplied_agents(agents).await;
    }

    pub async fn fire_session_start_hooks(
        &self,
        source: coco_hooks::orchestration::SessionStartSource,
    ) {
        self.runtime.fire_session_start_hooks(source).await;
    }

    pub fn set_client_hook_callback(&self, callback: coco_hooks::ClientHookCallback) {
        self.runtime.set_client_hook_callback(callback);
    }

    pub fn register_hook_definitions<I>(&self, hooks: I) -> usize
    where
        I: IntoIterator<Item = coco_hooks::HookDefinition>,
    {
        self.runtime.register_hook_definitions(hooks)
    }

    pub async fn wrap_send_elicitation_with_hooks(
        &self,
        server_name: String,
        base: coco_mcp::SendElicitation,
    ) -> coco_mcp::SendElicitation {
        self.runtime
            .wrap_send_elicitation_with_hooks(server_name, base)
            .await
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

    pub async fn clear_awaiting_plan_approval_if_matches(&self, request_id: &str) -> bool {
        self.runtime
            .clear_awaiting_plan_approval_if_matches(request_id)
            .await
    }

    pub async fn has_exited_plan_mode(&self) -> bool {
        self.runtime.has_exited_plan_mode().await
    }

    pub async fn set_agent_progress_summaries_enabled(&self, enabled: bool) {
        self.runtime
            .set_agent_progress_summaries_enabled(enabled)
            .await;
    }

    pub fn configured_plans_dir(&self) -> std::path::PathBuf {
        self.runtime.configured_plans_dir()
    }

    pub fn session_plan_file_path(&self) -> std::path::PathBuf {
        self.runtime.session_plan_file_path()
    }

    pub fn unscoped_session_plan_text(&self, session_id: &coco_types::SessionId) -> Option<String> {
        self.runtime.unscoped_session_plan_text(session_id)
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

    pub async fn initialize_metadata_snapshot(&self) -> super::SessionInitializeMetadata {
        let command_registry = self.current_command_registry().await;
        let commands = command_registry
            .client_visible()
            .iter()
            .map(|cmd| super::SessionInitializeCommand {
                name: cmd.base.name.clone(),
                description: cmd.base.description.clone(),
                argument_hint: cmd.base.argument_hint.clone().unwrap_or_default(),
            })
            .collect();

        let agent_catalog = self.agent_catalog_snapshot().await;
        let mut agents: Vec<super::SessionInitializeAgent> = agent_catalog
            .active()
            .cloned()
            .map(agent_definition_to_initialize_agent)
            .collect();
        agents.sort_by(|a, b| a.name.cmp(&b.name));

        let engine_config = self.current_engine_config().await;
        let cwd = engine_config.workspace_cwd();
        let plugin_style_sources = self.project_services().output_style_sources();
        let output_style_manager = crate::headless::build_output_style_manager(
            self.runtime_config(),
            &cwd,
            &plugin_style_sources,
        );
        let output_style = output_style_manager.active_name_for_initialize();
        let mut available_output_styles = output_style_manager.names();
        if !available_output_styles
            .iter()
            .any(|name| name == coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME)
        {
            available_output_styles.insert(0, coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME.into());
        }

        super::SessionInitializeMetadata {
            commands,
            agents,
            output_style,
            available_output_styles,
        }
    }

    pub async fn fast_mode_state(&self) -> coco_types::FastModeState {
        if self.current_engine_config().await.fast_mode {
            coco_types::FastModeState::On
        } else {
            coco_types::FastModeState::Off
        }
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

    pub async fn set_agent_color(&self, color: Option<coco_types::AgentColorName>) {
        self.runtime.set_agent_color(color).await;
    }

    pub async fn clear_replacement_snapshot(&self) -> super::ClearReplacementSnapshot {
        self.runtime.clear_replacement_snapshot().await
    }

    pub(crate) async fn apply_clear_replacement_snapshot(
        &self,
        snapshot: super::ClearReplacementSnapshot,
    ) {
        self.runtime
            .apply_clear_replacement_snapshot(snapshot)
            .await;
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

    pub async fn reload_plugin_environment(&self) -> super::SessionPluginReloadReport {
        self.runtime.reload_plugin_environment().await
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

    pub(crate) fn next_turn_id(&self) -> coco_types::TurnId {
        self.runtime.turn_coordinator.next_turn_id(&self.session_id)
    }

    pub(crate) fn reset_session_accounting(&self) {
        self.runtime.turn_coordinator.reset_accounting();
    }

    pub(crate) fn session_accounting_snapshot(&self) -> super::SessionAccounting {
        self.runtime.turn_coordinator.accounting_snapshot()
    }

    pub(crate) fn accumulate_session_result(&self, params: &coco_types::SessionResultParams) {
        self.runtime.turn_coordinator.accumulate_result(params);
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

    pub async fn mcp_status_result(&self) -> Option<coco_types::McpStatusResult> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        let names = manager.registered_server_names();
        let mut statuses = Vec::with_capacity(names.len());
        for name in &names {
            let state = manager.get_state(name).await;
            let (status, error, advertised) = match state {
                Some(coco_mcp::McpConnectionState::Connected(server)) => (
                    coco_types::McpConnectionStatus::Connected,
                    None,
                    server.tools.len() as i32,
                ),
                Some(coco_mcp::McpConnectionState::Pending { .. }) => {
                    (coco_types::McpConnectionStatus::Pending, None, 0)
                }
                Some(coco_mcp::McpConnectionState::Failed { error }) => {
                    (coco_types::McpConnectionStatus::Failed, Some(error), 0)
                }
                Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
                    (coco_types::McpConnectionStatus::NeedsAuth, None, 0)
                }
                Some(coco_mcp::McpConnectionState::Disabled) => {
                    (coco_types::McpConnectionStatus::Disabled, None, 0)
                }
                None => (coco_types::McpConnectionStatus::Disconnected, None, 0),
            };
            let registration = self.mcp_registration_status(name).await;
            statuses.push(coco_types::McpServerStatus {
                name: name.clone(),
                status,
                tool_count: registration
                    .as_ref()
                    .map_or(advertised, |report| report.tool_count),
                error,
                skipped_tools: registration
                    .as_ref()
                    .map_or_else(Vec::new, |report| report.skipped_tools.clone()),
                tombstoned_tools: registration
                    .map_or_else(Vec::new, |report| report.tombstoned_tools),
            });
        }
        Some(coco_types::McpStatusResult {
            mcp_servers: statuses,
        })
    }

    pub async fn set_dynamic_mcp_servers(
        &self,
        servers: Vec<(String, coco_mcp::McpServerConfig)>,
    ) -> Option<Vec<String>> {
        let manager = self.runtime.current_mcp_manager().await?;
        let mut manager = manager.lock().await;
        let mut added = Vec::with_capacity(servers.len());
        for (name, config) in servers {
            manager.register_server(coco_mcp::ScopedMcpServerConfig {
                name: name.clone(),
                config,
                scope: coco_mcp::ConfigScope::Dynamic,
                plugin_source: None,
            });
            added.push(name);
        }
        Some(added)
    }

    pub async fn install_client_mcp_route(&self, route: coco_mcp::ClientRouteMessage) -> bool {
        let Some(manager) = self.runtime.current_mcp_manager().await else {
            return false;
        };
        manager.lock().await.set_client_route_message(route);
        true
    }

    pub async fn register_client_mcp_servers(&self, server_names: &[String]) -> bool {
        let Some(manager) = self.runtime.current_mcp_manager().await else {
            return false;
        };
        let mut manager = manager.lock().await;
        for name in server_names {
            manager.register_server(coco_mcp::ScopedMcpServerConfig {
                name: name.clone(),
                config: coco_mcp::McpServerConfig::ClientHosted(
                    coco_mcp::types::McpClientHostedConfig { name: name.clone() },
                ),
                scope: coco_mcp::ConfigScope::Dynamic,
                plugin_source: None,
            });
        }
        true
    }

    pub async fn reconnect_mcp_server(
        &self,
        server_name: &str,
        send_elicitation: coco_mcp::SendElicitation,
    ) -> Option<super::SessionMcpConnectionChange> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        manager.disconnect(server_name).await;
        Some(
            self.connect_mcp_server_with_manager(&manager, server_name, send_elicitation)
                .await,
        )
    }

    pub async fn set_mcp_server_enabled(
        &self,
        server_name: &str,
        enabled: bool,
        send_elicitation: Option<coco_mcp::SendElicitation>,
    ) -> Option<super::SessionMcpConnectionChange> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        if enabled {
            let send_elicitation = send_elicitation?;
            Some(
                self.connect_mcp_server_with_manager(&manager, server_name, send_elicitation)
                    .await,
            )
        } else {
            manager.disconnect(server_name).await;
            self.deregister_mcp_server(server_name).await;
            Some(super::SessionMcpConnectionChange::Disconnected)
        }
    }

    async fn connect_mcp_server_with_manager(
        &self,
        manager: &coco_mcp::McpConnectionManager,
        server_name: &str,
        send_elicitation: coco_mcp::SendElicitation,
    ) -> super::SessionMcpConnectionChange {
        match manager.connect(server_name, send_elicitation).await {
            Ok(()) => {
                let schemas = collect_connected_mcp_server_schemas(manager, server_name).await;
                self.register_mcp_tools(server_name, schemas).await;
                super::SessionMcpConnectionChange::Connected
            }
            Err(error) => {
                if let Some((transport, url)) =
                    mcp_needs_auth_descriptor(manager, server_name).await
                {
                    self.register_mcp_auth_tool(server_name, &transport, url.as_deref());
                    super::SessionMcpConnectionChange::NeedsAuth { transport, url }
                } else {
                    super::SessionMcpConnectionChange::Failed(error.to_string())
                }
            }
        }
    }

    pub async fn register_mcp_tools(
        &self,
        server_name: &str,
        schemas: Vec<coco_tool_runtime::McpToolSchema>,
    ) {
        let report = coco_tools::register_mcp_tools(self.tools(), server_name, schemas);
        self.record_mcp_registration_report(server_name, report)
            .await;
    }

    pub fn register_mcp_auth_tool(&self, server_name: &str, transport: &str, url: Option<&str>) {
        coco_tools::register_mcp_auth_tool(self.tools(), server_name, transport, url);
    }

    pub async fn deregister_mcp_server(&self, server_name: &str) {
        coco_tools::deregister_mcp_server(self.tools(), server_name);
        self.clear_mcp_registration_status(server_name).await;
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

    pub fn set_sandbox_approval_bridge(
        &self,
        approval_bridge: coco_sandbox::SandboxApprovalBridgeRef,
    ) -> bool {
        let Some(sandbox_state) = self.sandbox_state() else {
            return false;
        };
        sandbox_state.set_approval_bridge(approval_bridge);
        true
    }

    pub async fn install_sandbox_reload_supervisor(&self) -> bool {
        let Some(sandbox_state) = self.sandbox_state() else {
            return false;
        };
        let Some(publisher) = self.runtime_publisher() else {
            return false;
        };
        self.install_reload_supervisor(crate::sandbox_reload::spawn_sandbox_reload(
            sandbox_state,
            &publisher,
            self.original_cwd().clone(),
        ))
        .await;
        true
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

async fn collect_connected_mcp_server_schemas(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Vec<coco_tool_runtime::McpToolSchema> {
    let Some(coco_mcp::McpConnectionState::Connected(server)) =
        manager.get_state(server_name).await
    else {
        return vec![];
    };
    server
        .tools
        .iter()
        .map(|tool| coco_tool_runtime::McpToolSchema {
            server_name: server_name.to_string(),
            tool_name: tool.name.clone(),
            description: tool.description.clone(),
            annotations: coco_tool_runtime::McpToolAnnotations::from_input_schema_meta(
                &tool.input_schema,
            ),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

async fn mcp_needs_auth_descriptor(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Option<(String, Option<String>)> {
    match manager.get_state(server_name).await {
        Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
            manager.auth_descriptor(server_name)
        }
        _ => None,
    }
}

fn agent_definition_to_initialize_agent(
    def: coco_types::AgentDefinition,
) -> super::SessionInitializeAgent {
    super::SessionInitializeAgent {
        name: def.name,
        description: def.description.unwrap_or_default(),
        model: def.model,
    }
}
