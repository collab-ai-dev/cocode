use super::*;

impl SessionHandle {
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

    /// Typed control for the per-turn working-directory override. Exposed so
    /// callers set this one field without reaching for an arbitrary
    /// config-mutation closure.
    pub async fn set_cwd_override(&self, cwd: Option<std::path::PathBuf>) {
        self.update_engine_config(move |c| c.cwd_override = cwd)
            .await;
    }

    /// Crate-internal escape hatch for arbitrary engine-config edits. The
    /// public surface exposes only typed controls (`set_model_id`,
    /// `set_permission_mode`, `set_thinking_level`, `set_cwd_override`, …) so
    /// external callers cannot mutate arbitrary config fields through a closure.
    pub(crate) async fn update_engine_config<F>(&self, update: F)
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

    pub async fn clear_awaiting_plan_approval_if_matches(&self, request_id: &str) -> bool {
        self.runtime
            .clear_awaiting_plan_approval_if_matches(request_id)
            .await
    }

    pub async fn set_agent_progress_summaries_enabled(&self, enabled: bool) {
        self.runtime
            .set_agent_progress_summaries_enabled(enabled)
            .await;
    }

    pub async fn set_agent_color(&self, color: Option<coco_types::AgentColorName>) {
        self.runtime.set_agent_color(color).await;
    }

    /// Read the live agent color from app state without exposing the
    /// underlying lock.
    pub async fn agent_color(&self) -> Option<coco_types::AgentColorName> {
        self.runtime.app_state().read().await.agent_color
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
}
