use super::*;

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    pub async fn turn_start<Handler>(
        &self,
        handler: &Handler,
        params: TurnStartParams,
    ) -> Result<TurnStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TurnStart(params))
            .await
    }

    pub async fn turn_interrupt<Handler>(
        &self,
        handler: &Handler,
        target: SessionTarget,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TurnInterrupt(target))
            .await
    }

    pub async fn query_session<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
        mut params: TurnStartParams,
    ) -> Result<TurnStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        params.target = session.session_target();
        self.turn_start(handler, params).await
    }

    pub async fn interrupt_session<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.turn_interrupt(handler, session.session_target()).await
    }
}
