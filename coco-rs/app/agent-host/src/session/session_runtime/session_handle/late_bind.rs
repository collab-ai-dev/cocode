use super::*;

impl SessionHandle {
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
}
