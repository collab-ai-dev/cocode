use super::*;

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    pub async fn session_rename<Handler>(
        &self,
        handler: &Handler,
        params: SessionRenameParams,
    ) -> Result<SessionRenameResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionRename(params))
            .await
    }

    pub async fn session_toggle_tag<Handler>(
        &self,
        handler: &Handler,
        params: SessionToggleTagParams,
    ) -> Result<SessionToggleTagResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionToggleTag(params))
            .await
    }

    pub async fn session_cost<Handler>(
        &self,
        handler: &Handler,
        target: SessionTarget,
    ) -> Result<SessionCostResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionCost(target))
            .await
    }

    pub async fn session_status<Handler>(
        &self,
        handler: &Handler,
        target: SessionTarget,
    ) -> Result<SessionStatusResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionStatus(target))
            .await
    }

    pub async fn rewind_files<Handler>(
        &self,
        handler: &Handler,
        params: RewindFilesParams,
    ) -> Result<RewindFilesResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::RewindFiles(params))
            .await
    }

    pub async fn mcp_status<Handler>(
        &self,
        handler: &Handler,
        target: SessionTarget,
    ) -> Result<McpStatusResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpStatus(target))
            .await
    }

    pub async fn context_usage<Handler>(
        &self,
        handler: &Handler,
        target: SessionTarget,
    ) -> Result<ContextUsageResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ContextUsage(target))
            .await
    }

    pub async fn mcp_set_servers<Handler>(
        &self,
        handler: &Handler,
        params: McpSetServersParams,
    ) -> Result<McpSetServersResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpSetServers(params))
            .await
    }

    pub async fn mcp_reconnect<Handler>(
        &self,
        handler: &Handler,
        params: McpReconnectParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpReconnect(params))
            .await
    }

    pub async fn mcp_toggle<Handler>(
        &self,
        handler: &Handler,
        params: McpToggleParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpToggle(params))
            .await
    }

    pub async fn plugin_reload<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<PluginReloadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(
            handler,
            ClientRequest::PluginReload(session.session_target()),
        )
        .await
    }

    pub async fn hook_reload<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<HookReloadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::HookReload(session.session_target()))
            .await
    }
}
