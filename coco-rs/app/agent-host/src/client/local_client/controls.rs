use super::*;

impl<H: Clone> LocalServerClient<H> {
    pub async fn approval_resolve<Handler>(
        &self,
        handler: &Handler,
        params: ApprovalResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ApprovalResolve(params))
            .await
    }

    pub async fn user_input_resolve<Handler>(
        &self,
        handler: &Handler,
        params: UserInputResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::UserInputResolve(params))
            .await
    }

    pub async fn elicitation_resolve<Handler>(
        &self,
        handler: &Handler,
        params: ElicitationResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ElicitationResolve(params))
            .await
    }

    pub async fn set_model<Handler>(
        &self,
        handler: &Handler,
        params: SetModelParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetModel(params))
            .await
    }

    pub async fn set_model_role<Handler>(
        &self,
        handler: &Handler,
        params: SetModelRoleParams,
    ) -> Result<SetModelRoleResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetModelRole(params))
            .await
    }

    pub async fn set_permission_mode<Handler>(
        &self,
        handler: &Handler,
        params: SetPermissionModeParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetPermissionMode(params))
            .await
    }

    pub async fn set_thinking<Handler>(
        &self,
        handler: &Handler,
        params: SetThinkingParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetThinking(params))
            .await
    }

    pub async fn set_agent_color<Handler>(
        &self,
        handler: &Handler,
        params: SetAgentColorParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetAgentColor(params))
            .await
    }

    pub async fn apply_permission_update<Handler>(
        &self,
        handler: &Handler,
        params: ApplyPermissionUpdateParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ApplyPermissionUpdate(params))
            .await
    }

    pub async fn reset_session_permission_rules<Handler>(
        &self,
        handler: &Handler,
        session: &LocalSessionClient,
    ) -> Result<ResetSessionPermissionRulesResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(
            handler,
            ClientRequest::ResetSessionPermissionRules(session.interactive_target()),
        )
        .await
    }

    pub async fn update_env<Handler>(
        &self,
        handler: &Handler,
        params: UpdateEnvParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::UpdateEnv(params))
            .await
    }

    pub async fn cancel_request<Handler>(
        &self,
        handler: &Handler,
        params: CancelRequestParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::CancelRequest(params))
            .await
    }

    pub async fn config_read<Handler>(
        &self,
        handler: &Handler,
        params: ConfigReadParams,
    ) -> Result<ConfigReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigRead(params))
            .await
    }

    pub async fn config_write<Handler>(
        &self,
        handler: &Handler,
        params: ConfigWriteParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigWrite(params))
            .await
    }

    pub async fn config_apply_flags<Handler>(
        &self,
        handler: &Handler,
        params: ConfigApplyFlagsParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigApplyFlags(params))
            .await
    }
}
