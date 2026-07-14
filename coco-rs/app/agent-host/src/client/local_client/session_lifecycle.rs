use super::*;

impl<H: Clone> LocalServerClient<H> {
    pub async fn initialize<Handler>(
        &self,
        handler: &Handler,
        params: InitializeParams,
    ) -> Result<InitializeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::Initialize(params))
            .await
    }

    pub async fn keep_alive<Handler>(&self, handler: &Handler) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::KeepAlive)
            .await
    }

    pub async fn session_start<Handler>(
        &self,
        handler: &Handler,
        params: SessionStartParams,
    ) -> Result<SessionStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionStart(Box::new(params)))
            .await
    }

    pub async fn session_start_handle<Handler>(
        &self,
        handler: &Handler,
        params: SessionStartParams,
    ) -> Result<LocalSessionClient, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        let started = self.session_start(handler, params).await?;
        Ok(LocalSessionClient {
            session_id: started.session_id,
            surface_id: started.surface_id,
        })
    }

    pub async fn session_resume<Handler>(
        &self,
        handler: &Handler,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionResume(params))
            .await
    }

    pub async fn session_resume_handle<Handler>(
        &self,
        handler: &Handler,
        params: SessionResumeParams,
    ) -> Result<LocalSessionClient, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        let resumed = self.session_resume(handler, params).await?;
        Ok(LocalSessionClient {
            session_id: resumed.session.session_id,
            surface_id: resumed.surface_id,
        })
    }

    pub async fn session_subscribe<Handler>(
        &self,
        handler: &Handler,
        params: SessionSubscribeParams,
    ) -> Result<SessionSubscribeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionSubscribe(params))
            .await
    }

    pub async fn session_close<Handler>(
        &self,
        handler: &Handler,
        params: SessionCloseParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionClose(params))
            .await
    }

    pub async fn session_delete<Handler>(
        &self,
        handler: &Handler,
        params: SessionDeleteParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionDelete(params))
            .await
    }

    pub async fn session_replace<Handler>(
        &self,
        handler: &Handler,
        params: coco_types::SessionReplaceParams,
    ) -> Result<coco_types::SessionReplaceResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionReplace(Box::new(params)))
            .await
    }

    pub async fn close_session<Handler>(
        &mut self,
        handler: &Handler,
        session: LocalSessionClient,
    ) -> Result<(), (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        let params = SessionCloseParams {
            target: SessionCloseTarget::Interactive {
                target: session.interactive_target(),
            },
        };
        match self.session_close(handler, params).await {
            Ok(()) => {
                self.purge_surface_buffers(&session.surface_id);
                Ok(())
            }
            Err(error) => Err((session, error)),
        }
    }

    pub async fn replace_session_with_start<Handler>(
        &self,
        handler: &Handler,
        session: LocalSessionClient,
        params: SessionStartParams,
    ) -> Result<LocalSessionClient, (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        match self
            .session_replace(
                handler,
                coco_types::SessionReplaceParams {
                    source: session.interactive_target(),
                    destination: coco_types::SessionReplacement::Fresh(params),
                },
            )
            .await
        {
            Ok(replaced) => Ok(LocalSessionClient {
                session_id: replaced.session_id,
                surface_id: replaced.surface_id,
            }),
            Err(error) => Err((session, error)),
        }
    }

    pub async fn replace_session_with_resume<Handler>(
        &self,
        handler: &Handler,
        session: LocalSessionClient,
        params: SessionResumeParams,
    ) -> Result<LocalSessionClient, (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        match self
            .session_replace(
                handler,
                coco_types::SessionReplaceParams {
                    source: session.interactive_target(),
                    destination: coco_types::SessionReplacement::Resume(params.target),
                },
            )
            .await
        {
            Ok(replaced) => Ok(LocalSessionClient {
                session_id: replaced.session_id,
                surface_id: replaced.surface_id,
            }),
            Err(error) => Err((session, error)),
        }
    }

    pub async fn replace_session_with_clear<Handler>(
        &self,
        handler: &Handler,
        session: LocalSessionClient,
    ) -> Result<LocalSessionClient, (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        match self
            .session_replace(
                handler,
                coco_types::SessionReplaceParams {
                    source: session.interactive_target(),
                    destination: coco_types::SessionReplacement::Clear,
                },
            )
            .await
        {
            Ok(replaced) => Ok(LocalSessionClient {
                session_id: replaced.session_id,
                surface_id: replaced.surface_id,
            }),
            Err(error) => Err((session, error)),
        }
    }
}
