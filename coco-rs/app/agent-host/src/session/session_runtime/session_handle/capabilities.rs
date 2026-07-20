use super::*;

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

    /// Construction-time execution profile (`Primary` vs `SideChatReadOnly`).
    pub(crate) fn execution_profile(
        &self,
    ) -> crate::session::session_runtime::SessionExecutionProfile {
        self.runtime.execution_profile()
    }

    pub(crate) fn app_state(&self) -> &Arc<tokio::sync::RwLock<coco_types::ToolAppState>> {
        self.runtime.app_state()
    }

    pub(crate) async fn apply_side_chat_parent_state(
        &self,
        parent_config: coco_query::QueryEngineConfig,
        permissions: coco_types::LiveToolPermissionState,
    ) {
        self.runtime
            .update_engine_config(move |config| {
                config.model_id = parent_config.model_id;
                config.system_prompt = parent_config.system_prompt;
                config.append_system_prompt = parent_config.append_system_prompt;
                config.thinking_level = parent_config.thinking_level;
                config.fast_mode = parent_config.fast_mode;
                config.permission_mode = parent_config.permission_mode;
                config.wire_dump = parent_config.wire_dump;
            })
            .await;
        self.runtime.app_state().write().await.permissions = permissions;
    }

    pub(crate) async fn install_usage_mirror(
        &self,
        accounting: coco_query::usage_accounting::UsageAccounting,
        source: coco_types::UsageSource,
    ) {
        self.runtime
            .usage_accounting()
            .install_mirror(accounting, source)
            .await;
    }

    pub fn original_cwd(&self) -> &std::path::PathBuf {
        self.runtime.original_cwd()
    }

    pub fn project_root(&self) -> &std::path::PathBuf {
        self.runtime.project_root()
    }

    pub(crate) fn current_cwd(&self) -> &Arc<tokio::sync::RwLock<std::path::PathBuf>> {
        self.runtime.current_cwd()
    }

    pub fn config_home(&self) -> &std::path::PathBuf {
        self.runtime.config_home()
    }

    pub async fn prompt_history_entries(&self, project: String) -> Vec<coco_session::HistoryEntry> {
        self.runtime.prompt_history_entries(project).await
    }

    pub async fn persist_prompt_history_entry(
        &self,
        project: String,
        composer: coco_types::PersistedComposer,
    ) -> anyhow::Result<()> {
        self.runtime
            .persist_prompt_history_entry(project, composer)
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

    pub(crate) fn file_read_state(&self) -> &Arc<tokio::sync::RwLock<coco_context::FileReadState>> {
        self.runtime.file_read_state()
    }

    pub fn file_history_enabled(&self) -> bool {
        self.runtime.file_history().is_some()
    }

    pub fn hook_registry(&self) -> Arc<coco_hooks::HookRegistry> {
        self.runtime.hook_registry()
    }

    /// The session's first-class goal aggregate (§10.2). Sole writer of the live
    /// goal projection; control-plane commands and tools reach it through here.
    pub fn goal_runtime(&self) -> Arc<coco_goal_runtime::GoalRuntimeHandle> {
        self.runtime.goal_runtime().clone()
    }

    /// The session-scoped runtime-owned evidence store (§10.2 #9).
    pub fn goal_evidence(&self) -> Arc<dyn coco_goal_runtime::EvidenceStore> {
        self.runtime.goal_evidence().clone()
    }

    /// The goal continuation driver's cold-edge signal (§10.3); nudge it after a
    /// resume so the driver starts a turn for the now-active goal.
    pub fn goal_driver_edge(&self) -> Arc<tokio::sync::Notify> {
        self.runtime.goal_driver_edge().clone()
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

    /// The skill-learning review runtime, when the loop is enabled. Mirrors
    /// [`Self::memory_runtime`]; consumed by the `/learn` slash dispatcher.
    pub fn skill_review_runtime(&self) -> Option<&Arc<coco_skill_learn::SkillReviewRuntime>> {
        self.runtime.skill_review_runtime()
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

    pub(crate) fn loop_sentinel_state(
        &self,
    ) -> &Arc<tokio::sync::Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>> {
        self.runtime.loop_sentinel_state()
    }

    pub fn usage_accounting(&self) -> coco_query::usage_accounting::UsageAccounting {
        self.runtime.usage_accounting()
    }

    pub(crate) fn skill_bash_cell(
        &self,
    ) -> Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>> {
        self.runtime.skill_bash_cell()
    }

    pub(crate) fn agent_catalog_handle(
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

    pub async fn current_agent_handle(&self) -> Option<coco_tool_runtime::AgentHandleRef> {
        self.runtime.current_agent_handle().await
    }

    pub async fn current_agent_transcript_store(
        &self,
    ) -> Option<coco_tool_runtime::AgentTranscriptStoreRef> {
        self.runtime.current_agent_transcript_store().await
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
                first_prompt: history
                    .iter()
                    .filter(|message| matches!(message.as_ref(), coco_messages::Message::User(_)))
                    .filter_map(session_message_preview)
                    .find(|text| !is_synthetic_prompt(text))
                    .unwrap_or_default(),
                last_message_preview: history
                    .iter()
                    .rev()
                    .filter_map(session_message_preview)
                    .next(),
                message_count: history
                    .iter()
                    .filter(|message| {
                        matches!(
                            message.as_ref(),
                            coco_messages::Message::User(_) | coco_messages::Message::Assistant(_)
                        )
                    })
                    .count() as i32,
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

    pub async fn has_exited_plan_mode(&self) -> bool {
        self.runtime.has_exited_plan_mode().await
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

    pub async fn status_report(&self) -> String {
        self.runtime.status_report().await
    }

    pub fn resolve_model_selection(
        &self,
        value: &str,
    ) -> Option<coco_types::ProviderModelSelection> {
        self.runtime.resolve_model_selection(value)
    }

    /// Narrow append-only handle over this session's live permission-rule
    /// overlay. Returns a capability, not the raw lock, so the public API
    /// exposes an operation instead of leaking `Arc<RwLock<_>>`.
    pub fn live_permission_rules(&self) -> super::super::LivePermissionRulesHandle {
        self.runtime.live_permission_rules_handle()
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
            .remote_client_visible()
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
}

fn session_message_preview(message: &Arc<coco_messages::Message>) -> Option<String> {
    if !matches!(
        message.as_ref(),
        coco_messages::Message::User(_) | coco_messages::Message::Assistant(_)
    ) {
        return None;
    }
    let text = coco_messages::wrapping::extract_text_from_message(message);
    let flat = text.replace('\n', " ");
    let preview = coco_utils_string::truncate_str(flat.trim(), 200);
    (!preview.is_empty()).then_some(preview)
}

fn is_synthetic_prompt(text: &str) -> bool {
    let text = text.trim();
    text == coco_messages::INTERRUPT_MESSAGE
        || text == coco_messages::INTERRUPT_MESSAGE_FOR_TOOL_USE
        || text.starts_with("[Request interrupted by user")
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
