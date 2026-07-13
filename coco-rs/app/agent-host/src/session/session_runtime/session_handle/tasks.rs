use super::*;

impl SessionHandle {
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
}
