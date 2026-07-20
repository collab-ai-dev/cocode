use std::sync::Arc;

use coco_app_server_client::ClientError;
use coco_types::{
    CoreEvent, ServerNotification, SessionId, SessionLifecycleEffectKind, SessionScopedEvent,
    TuiOnlyEvent,
};
use tokio::sync::mpsc;

use super::super::session_close::close_local_app_server_session;
use super::super::session_registry::load_local_app_server_child_session;
use super::{AppServerLocalBridge, AppServerLocalSessionBinding, AppServerLocalTurnCompletion};

fn scope_session_event(session_id: SessionId, event: CoreEvent) -> Option<CoreEvent> {
    let event = match SessionScopedEvent::try_from(event) {
        Ok(event) => event,
        Err(_) => {
            tracing::warn!(%session_id, "dropping unscoped TUI event received from AppServer");
            return None;
        }
    };
    Some(CoreEvent::Tui(TuiOnlyEvent::SessionScoped {
        session_id,
        event: Box::new(event),
    }))
}

impl AppServerLocalBridge {
    /// Resolve a live runtime by exact id. Commands emitted by a stale TUI
    /// projection must never fall back to whichever session is current now.
    pub fn session_by_id(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::session_runtime::SessionHandle> {
        self.app_server
            .registry()
            .get(session_id)
            .map(crate::host::app_session::AppSessionHandle::into_session)
    }

    pub fn full_session_by_id(
        &self,
        session_id: &SessionId,
    ) -> Option<&crate::local_client::LocalSessionClient> {
        self.full_session
            .as_ref()
            .filter(|client| client.session_id() == session_id)
            .or_else(|| {
                self.child_full_session()
                    .filter(|client| client.session_id() == session_id)
            })
    }

    pub fn is_child_session(&self, session_id: &SessionId) -> bool {
        self.child_full_session()
            .is_some_and(|client| client.session_id() == session_id)
    }

    pub fn full_session(&self) -> Option<&crate::local_client::LocalSessionClient> {
        self.full_session.as_ref()
    }

    pub fn child_full_session(&self) -> Option<&crate::local_client::LocalSessionClient> {
        self.child_full_session.as_ref().filter(|client| {
            self.app_server.has_session_slot(client.session_id())
                || self
                    .child_event_pump
                    .as_ref()
                    .is_some_and(|pump| !pump.is_finished())
        })
    }

    async fn attach_child_full_session(
        &mut self,
        child: crate::session_runtime::SessionHandle,
        parent_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let child_id = child.session_id().clone();
        let client = self.client.attach_full_session(child_id.clone())?;
        if let Some(event_tx) = event_tx {
            self.start_child_event_pump(parent_id, child_id, event_tx);
        }
        self.child_full_session = Some(client.clone());
        Ok(AppServerLocalSessionBinding {
            session: child,
            client,
        })
    }

    fn start_child_event_pump(
        &mut self,
        parent_id: SessionId,
        child_id: SessionId,
        event_tx: mpsc::Sender<CoreEvent>,
    ) {
        if let Some(handle) = self.child_event_pump.take() {
            handle.abort();
        }
        let mut client = self.client.clone();
        let observed = client.observe_session(child_id.clone());
        let mut lifecycle_client = client.clone();
        let app_server = Arc::clone(&self.app_server);
        self.child_event_pump = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    envelope = client.next_session_event(&observed) => {
                        let Some(envelope) = envelope else { break };
                        let Some(event) = scope_session_event(envelope.session_id, envelope.event) else {
                            continue;
                        };
                        if event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    effect = lifecycle_client.next_lifecycle_effect() => {
                        let Some(effect) = effect else { break };
                        let child_ended = matches!(
                            &effect.kind,
                            SessionLifecycleEffectKind::SessionEnded { session_id }
                                if session_id == &child_id
                        );
                        if !child_ended {
                            continue;
                        }
                        if let Some(parent) = app_server.registry().get(&parent_id) {
                            let usage = parent.runtime().session_usage_snapshot().await;
                            let event = CoreEvent::Protocol(
                                ServerNotification::SessionUsageUpdated(Box::new(usage)),
                            );
                            if let Some(event) = scope_session_event(parent_id.clone(), event) {
                                let _ = event_tx.send(event).await;
                            }
                        }
                        let _ = event_tx
                            .send(CoreEvent::Tui(TuiOnlyEvent::SideChatExited {
                                parent_id: parent_id.clone(),
                                child_id: child_id.clone(),
                            }))
                            .await;
                        break;
                    }
                }
            }
        }));
    }

    pub async fn start_child_turn(
        &mut self,
        mut params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        let client = self
            .child_full_session
            .as_ref()
            .ok_or_else(|| ClientError::InvalidArgument("child full session missing".into()))?;
        params.target = client.session_target();
        self.client.turn_start(&self.handler, params).await
    }

    pub async fn interrupt_child_turn(&self) -> Result<bool, ClientError> {
        let Some(client) = self.child_full_session.as_ref() else {
            return Ok(false);
        };
        self.client.interrupt_session(&self.handler, client).await?;
        Ok(true)
    }

    pub async fn open_side_chat(
        &mut self,
        parent_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        if self.child_full_session().is_some() {
            return Err(ClientError::InvalidArgument(
                "a sidechat child is already active".into(),
            ));
        }
        let replacement = self
            .handler
            .state
            .runtime_replacement_snapshot()
            .await
            .ok_or_else(|| {
                ClientError::InvalidArgument(
                    "sidechat requires a configured runtime factory".into(),
                )
            })?;
        let parent = self
            .app_server
            .registry()
            .get(&parent_id)
            .ok_or_else(|| ClientError::InvalidArgument("sidechat parent is not live".into()))?
            .into_session();
        let child_id = SessionId::generate();
        let build_child_id = child_id.clone();
        let factory = async move {
            let seed = parent.capture_side_chat_seed().await.map_err(|error| {
                coco_app_server::RegistryError::load_failed(format!(
                    "sidechat context capture failed: {error:?}"
                ))
            })?;
            let child = replacement
                .runtime_factory
                .build_side_chat(Some(build_child_id), replacement.cwd.clone(), seed)
                .await
                .map_err(|error| {
                    coco_app_server::RegistryError::load_failed(format!(
                        "building sidechat child failed: {error:#}"
                    ))
                })?;
            Ok(crate::app_session::AppSessionHandle::from_runtime(child))
        };
        let handle = load_local_app_server_child_session(
            &self.app_server,
            Arc::clone(&self.handler.state),
            parent_id.clone(),
            child_id.clone(),
            factory,
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| {
            ClientError::InvalidArgument(format!("load sidechat child failed: {error:?}"))
        })?;
        let child = handle.into_session();
        crate::app_server_host::hook_callback_bridge::install_runtime_callback(
            Arc::clone(&self.app_server),
            &child,
        );
        match self
            .attach_child_full_session(child, parent_id, event_tx)
            .await
        {
            Ok(binding) => Ok(binding),
            Err(error) => {
                if let Err(cleanup_error) = self.close_registered_child(child_id).await {
                    tracing::error!(%cleanup_error, "failed to roll back sidechat attach");
                }
                Err(error)
            }
        }
    }

    async fn close_registered_child(&self, child_id: SessionId) -> Result<(), ClientError> {
        close_local_app_server_session(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            child_id,
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn close_child(&mut self) -> Result<Option<SessionId>, ClientError> {
        let Some(client) = self.child_full_session.as_ref() else {
            return Ok(None);
        };
        let child_id = client.session_id().clone();
        let close_result = self.close_registered_child(child_id.clone()).await;
        let mut terminal_pump = None;
        if !self.app_server.has_session_slot(&child_id) {
            terminal_pump = self.child_event_pump.take();
            self.child_full_session = None;
        }
        if let Some(pump) = terminal_pump {
            match tokio::time::timeout(self.handler.turn_drain_timeout, pump).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %child_id, "sidechat event pump join failed");
                }
                Err(_) => tracing::warn!(%child_id, "timed out waiting for sidechat exit event"),
            }
        }
        close_result?;
        Ok(Some(child_id))
    }

    pub fn ensure_full_session(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        if self
            .full_session
            .as_ref()
            .is_some_and(|client| client.session_id() == &session_id)
        {
            return Ok(());
        }
        let already_attached = {
            let routing = self
                .app_server
                .routing()
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            routing
                .attachment(self.client.connection_key(), &session_id)
                .is_some()
                && routing.session_access(self.client.connection_key(), &session_id)
                    == Some(coco_types::SessionAccess::Full)
        };
        let client = if already_attached {
            self.client.observe_session(session_id)
        } else {
            self.client.attach_full_session(session_id)?
        };
        self.full_session = Some(client);
        Ok(())
    }

    pub fn start_event_pump(&mut self, session_id: SessionId, event_tx: mpsc::Sender<CoreEvent>) {
        if self
            .event_pump_session_id
            .as_ref()
            .is_some_and(|active| active == &session_id)
        {
            return;
        }
        if let Some(handle) = self.event_pump.take() {
            handle.abort();
        }
        let mut client = self.client.clone();
        let observed = client.observe_session(session_id.clone());
        self.event_pump = Some(tokio::spawn(async move {
            while let Some(envelope) = client.next_session_event(&observed).await {
                let Some(event) = scope_session_event(envelope.session_id, envelope.event) else {
                    continue;
                };
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        }));
        self.event_pump_session_id = Some(session_id);
    }

    pub async fn bind_full_session(
        &mut self,
        session: crate::session_runtime::SessionHandle,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<(), ClientError> {
        let session_id = session.session_id().clone();
        self.register_session_runtime(session).await;
        self.ensure_full_session(session_id.clone())?;
        if let Some(event_tx) = event_tx {
            self.start_event_pump(session_id, event_tx);
        }
        Ok(())
    }

    pub fn activate_existing_full_session(
        &mut self,
        session_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<(), ClientError> {
        self.ensure_full_session(session_id.clone())?;
        if let Some(event_tx) = event_tx {
            self.start_event_pump(session_id, event_tx);
        }
        Ok(())
    }

    pub async fn start_session(
        &mut self,
        params: coco_types::SessionStartParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let client = self
            .client
            .session_start_handle(&self.handler, params)
            .await?;
        self.bind_lifecycle_session(client, event_tx)
    }

    pub async fn resume_session(
        &mut self,
        params: coco_types::SessionResumeParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let client = self
            .client
            .session_resume_handle(&self.handler, params)
            .await?;
        self.bind_lifecycle_session(client, event_tx)
    }

    pub async fn replace_session_with_resume(
        &mut self,
        params: coco_types::SessionResumeParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let current = self
            .full_session
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("full session missing".into()))?;
        if current.session_id() == &params.target.session_id {
            return self.resume_session(params, event_tx).await;
        }
        let client = self
            .client
            .replace_session_with_resume(&self.handler, current, params)
            .await
            .map_err(|(_session, error)| error)?;
        self.bind_lifecycle_session(client, event_tx)
    }

    pub async fn replace_session_with_clear(
        &mut self,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let current = self
            .full_session
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("full session missing".into()))?;
        let client = self
            .client
            .replace_session_with_clear(&self.handler, current)
            .await
            .map_err(|(_session, error)| error)?;
        self.bind_lifecycle_session(client, event_tx)
    }

    fn bind_lifecycle_session(
        &mut self,
        client: crate::local_client::LocalSessionClient,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let session_id = client.session_id().clone();
        let handle = self.app_server.registry().get(&session_id).ok_or_else(|| {
            ClientError::InvalidArgument(format!(
                "local lifecycle returned unregistered session {session_id}"
            ))
        })?;
        let session = handle.into_session();
        self.full_session = Some(client.clone());
        if let Some(event_tx) = event_tx {
            self.start_event_pump(session_id, event_tx);
        }
        Ok(AppServerLocalSessionBinding { session, client })
    }

    pub async fn start_turn_and_wait_for_end(
        &mut self,
        session_id: SessionId,
        mut params: coco_types::TurnStartParams,
    ) -> Result<AppServerLocalTurnCompletion, ClientError> {
        self.ensure_full_session(session_id)?;
        let session = self
            .full_session
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("full session missing".into()))?;
        params.target = session.session_target();
        let started = self.client.turn_start(&self.handler, params).await?;
        loop {
            let Some(envelope) = self.client.next_session_event(&session).await else {
                return Err(ClientError::Disconnected);
            };
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == started.turn_id
            {
                let session_result = ended.session_result.as_deref().cloned().ok_or_else(|| {
                    ClientError::InvalidArgument(format!(
                        "turn {} ended without per-turn session_result",
                        started.turn_id
                    ))
                })?;
                return Ok(AppServerLocalTurnCompletion {
                    started,
                    ended,
                    session_result,
                });
            }
        }
    }

    pub async fn start_turn(
        &mut self,
        session_id: SessionId,
        mut params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        self.ensure_full_session(session_id)?;
        let session = self
            .full_session
            .as_ref()
            .ok_or_else(|| ClientError::InvalidArgument("full session missing".into()))?;
        params.target = session.session_target();
        self.client.turn_start(&self.handler, params).await
    }
}
