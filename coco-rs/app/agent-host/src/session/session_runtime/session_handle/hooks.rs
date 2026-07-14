use super::*;

impl SessionHandle {
    pub async fn reload_plugins(&self, cwd: &std::path::Path) -> usize {
        self.runtime.reload_plugins(cwd).await
    }

    pub async fn reload_agent_catalog(&self) {
        self.runtime.reload_agent_catalog().await;
    }

    pub async fn reload_lsp_servers(&self) {
        self.runtime.reload_lsp_servers().await;
    }

    pub async fn reload_hooks(&self) -> anyhow::Result<usize> {
        self.runtime.reload_hooks().await
    }

    pub async fn set_client_supplied_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        self.runtime.set_client_supplied_agents(agents).await;
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
}
