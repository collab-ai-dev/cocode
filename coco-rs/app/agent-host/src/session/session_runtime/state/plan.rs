use super::*;

impl SessionRuntime {
    pub async fn clear_awaiting_plan_approval_if_matches(&self, request_id: &str) -> bool {
        let mut guard = self.engine_state_resources.app_state().write().await;
        if guard.awaiting_plan_approval_request_id.as_deref() != Some(request_id) {
            return false;
        }
        guard.awaiting_plan_approval = false;
        guard.awaiting_plan_approval_request_id = None;
        true
    }
    pub async fn has_exited_plan_mode(&self) -> bool {
        self.engine_state_resources
            .app_state()
            .read()
            .await
            .has_exited_plan_mode
    }
    pub fn configured_plans_dir(&self) -> std::path::PathBuf {
        coco_context::resolve_plans_directory(
            self.config_home(),
            self.runtime_config().paths.project_dir.as_deref(),
            self.runtime_config()
                .settings
                .merged
                .plans_directory
                .as_deref(),
        )
    }
    pub fn session_plan_file_path(&self) -> std::path::PathBuf {
        let plans_dir = self.configured_plans_dir();
        let session_id = self.current_typed_session_id_snapshot();
        coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None)
    }
    pub fn unscoped_session_plan_text(&self, session_id: &coco_types::SessionId) -> Option<String> {
        let plans_dir = coco_context::resolve_plans_directory(
            self.config_home(),
            /*project_dir*/ None,
            /*setting*/ None,
        );
        coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None)
    }
}
