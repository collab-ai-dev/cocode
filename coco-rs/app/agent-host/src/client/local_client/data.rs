use super::*;

impl<H: Clone> LocalServerClient<H> {
    pub async fn session_list<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<SessionListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionList)
            .await
    }

    pub async fn session_read<Handler>(
        &self,
        handler: &Handler,
        params: SessionReadParams,
    ) -> Result<SessionReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionRead(params))
            .await
    }

    pub async fn session_turns_list<Handler>(
        &self,
        handler: &Handler,
        params: SessionTurnsListParams,
    ) -> Result<SessionTurnsListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionTurnsList(params))
            .await
    }

    pub async fn read_passive_session<Handler>(
        &self,
        handler: &Handler,
        session: &LocalPassiveSessionClient,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.session_read(
            handler,
            SessionReadParams {
                target: session.session_target(),
                cursor,
                limit,
            },
        )
        .await
    }

    pub async fn list_passive_session_turns<Handler>(
        &self,
        handler: &Handler,
        session: &LocalPassiveSessionClient,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionTurnsListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.session_turns_list(
            handler,
            SessionTurnsListParams {
                target: session.session_target(),
                cursor,
                limit,
            },
        )
        .await
    }
}
