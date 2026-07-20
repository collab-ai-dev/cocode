use super::*;

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    pub async fn stop_task<Handler>(
        &self,
        handler: &Handler,
        params: StopTaskParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::StopTask(params))
            .await
    }

    pub async fn task_list<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<TaskListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TaskList(session.session_target()))
            .await
    }

    pub async fn task_detail<Handler>(
        &self,
        handler: &Handler,
        params: TaskDetailParams,
    ) -> Result<TaskDetailResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TaskDetail(params))
            .await
    }

    pub async fn background_all_tasks<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<BackgroundAllTasksResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(
            handler,
            ClientRequest::BackgroundAllTasks(session.session_target()),
        )
        .await
    }

    pub async fn agent_interrupt_current_work<Handler>(
        &self,
        handler: &Handler,
        params: AgentInterruptCurrentWorkParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::AgentInterruptCurrentWork(params))
            .await
    }
}
