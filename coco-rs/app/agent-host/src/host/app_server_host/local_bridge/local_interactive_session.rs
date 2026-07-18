use std::sync::Arc;

use coco_app_server::{AttachSurfaceOptions, LocalClientAdapter, LocalClientInbound};
use coco_app_server_client::ClientError;
use coco_types::{
    CoreEvent, ServerNotification, SessionId, SessionScopedEvent, SurfaceId,
    SurfaceLifecycleEffectKind, TuiOnlyEvent,
};
use tokio::sync::mpsc;

use super::super::session_close::close_local_app_server_session;
use super::super::session_registry::load_local_app_server_child_session;
use super::{AppServerLocalBridge, AppServerLocalSessionBinding, AppServerLocalTurnCompletion};

fn scope_surface_event(session_id: SessionId, event: CoreEvent) -> Option<CoreEvent> {
    let event = match SessionScopedEvent::try_from(event) {
        Ok(event) => event,
        Err(_) => {
            tracing::warn!(
                %session_id,
                "dropping TUI-only event received from an AppServer surface"
            );
            return None;
        }
    };
    Some(CoreEvent::Tui(TuiOnlyEvent::SessionScoped {
        session_id,
        event: Box::new(event),
    }))
}

impl AppServerLocalBridge {
    pub fn interactive_session(&self) -> Option<&crate::local_client::LocalSessionClient> {
        self.interactive_surface.as_ref()
    }

    pub fn child_interactive_session(&self) -> Option<&crate::local_client::LocalSessionClient> {
        self.child_interactive_surface.as_ref().filter(|surface| {
            // The lifecycle pump may observe an autonomous child close before
            // the next mutable bridge call gets a chance to clear the cached
            // handle. Keep reporting the child until its terminal exit event
            // has actually been queued; after that, registry membership is the
            // authority boundary and a terminal surface is never exposed as a
            // live sidechat.
            self.app_server.has_session_slot(surface.session_id())
                || self
                    .child_event_pump
                    .as_ref()
                    .is_some_and(|pump| !pump.is_finished())
        })
    }

    async fn attach_child_interactive_session(
        &mut self,
        child: crate::session_runtime::SessionHandle,
        parent_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let child_id = child.session_id().clone();
        let surface = self
            .client
            .attach_interactive_session(child_id.clone(), AttachSurfaceOptions::default())?;
        if let Some(event_tx) = event_tx {
            self.start_child_event_pump(parent_id, child_id, event_tx)?;
        }
        self.child_interactive_surface = Some(surface.clone());
        Ok(AppServerLocalSessionBinding {
            session: child,
            surface,
        })
    }

    fn start_child_event_pump(
        &mut self,
        parent_id: SessionId,
        child_id: SessionId,
        event_tx: mpsc::Sender<CoreEvent>,
    ) -> Result<(), ClientError> {
        if let Some(handle) = self.child_event_pump.take() {
            handle.abort();
        }
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        let mut connection = adapter.connect();
        let surface = connection
            .attach_surface(child_id.clone(), AttachSurfaceOptions::default())
            .map_err(crate::local_client::client_error_from_attach)?;
        let surface_id = surface.surface_id;
        let app_server = Arc::clone(&self.app_server);
        self.child_event_pump = Some(tokio::spawn(async move {
            while let Some(inbound) = connection.recv().await {
                match inbound {
                    LocalClientInbound::Event(delivery) if delivery.surface_id == surface_id => {
                        let delivery = *delivery;
                        let Some(event) = scope_surface_event(
                            delivery.envelope.session_id,
                            delivery.envelope.event,
                        ) else {
                            continue;
                        };
                        if event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    LocalClientInbound::Lifecycle(effect) if effect.surface_id == surface_id => {
                        let child_ended = matches!(
                            &effect.kind,
                            SurfaceLifecycleEffectKind::SessionEnded { session_id }
                                if session_id == &child_id
                        );
                        if child_ended {
                            if let Some(parent) = app_server.registry().get(&parent_id) {
                                let usage = parent.runtime().session_usage_snapshot().await;
                                let event = CoreEvent::Protocol(
                                    ServerNotification::SessionUsageUpdated(Box::new(usage)),
                                );
                                if let Some(event) = scope_surface_event(parent_id.clone(), event) {
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
                    LocalClientInbound::Event(_) | LocalClientInbound::Lifecycle(_) => {}
                }
            }
        }));
        Ok(())
    }

    /// Start a turn on the sidechat child (its own turn coordinator — never
    /// touches the parent's).
    pub async fn start_child_turn(
        &mut self,
        mut params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        let surface = self.child_interactive_surface.as_ref().ok_or_else(|| {
            ClientError::InvalidArgument("child interactive surface missing".into())
        })?;
        params.target = surface.interactive_target();
        self.client.turn_start(&self.handler, params).await
    }

    pub async fn interrupt_child_turn(&self) -> Result<bool, ClientError> {
        let Some(surface) = self.child_interactive_surface.as_ref() else {
            return Ok(false);
        };
        self.client
            .interrupt_session(&self.handler, surface)
            .await?;
        Ok(true)
    }

    /// Reserve, build, and attach a sidechat child of `parent_id` without
    /// starting a turn. The caller can switch the TUI projection using the
    /// returned ids before the child's first event-producing turn begins.
    pub async fn open_side_chat(
        &mut self,
        parent_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        if self.child_interactive_session().is_some() {
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
                .build_side_chat(
                    Some(build_child_id),
                    replacement.cwd.clone(),
                    Default::default(),
                    seed,
                )
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
            parent_id.clone(),
            child_id.clone(),
            factory,
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
            .attach_child_interactive_session(child, parent_id, event_tx)
            .await
        {
            Ok(binding) => Ok(binding),
            Err(error) => {
                let cleanup = self.close_registered_child(child_id).await;
                if let Err(cleanup_error) = cleanup {
                    tracing::error!(%cleanup_error, "failed to roll back sidechat after attach failure");
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

    /// Close the sidechat child without leaving stale bridge authority.
    /// Handles remain installed if close fails before the registry commits;
    /// once the slot is terminally removed they are cleared even when runtime
    /// teardown reported an error. Once removal commits, this waits for the
    /// lifecycle pump to publish the parent usage snapshot and exit event so
    /// callers cannot race a second, weaker transition into the TUI.
    pub async fn close_child(&mut self) -> Result<Option<SessionId>, ClientError> {
        let Some(surface) = self.child_interactive_surface.as_ref() else {
            return Ok(None);
        };
        let child_id = surface.session_id().clone();
        let close_result = self.close_registered_child(child_id.clone()).await;
        let mut terminal_pump = None;
        if !self.app_server.has_session_slot(&child_id) {
            terminal_pump = self.child_event_pump.take();
            self.child_interactive_surface = None;
        }
        if let Some(pump) = terminal_pump {
            match tokio::time::timeout(self.handler.turn_drain_timeout, pump).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %child_id, "sidechat lifecycle pump join failed");
                }
                Err(_) => {
                    tracing::warn!(%child_id, "timed out waiting for sidechat lifecycle event");
                }
            }
        }
        close_result?;
        Ok(Some(child_id))
    }

    pub fn ensure_interactive_surface(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        if self
            .interactive_surface
            .as_ref()
            .is_some_and(|surface| surface.session_id() == &session_id)
        {
            return Ok(());
        }
        let can_repoint = self.interactive_surface.as_ref().is_some_and(|surface| {
            self.surface_is_attached_to_session(surface.surface_id(), &session_id)
        });
        if can_repoint {
            // Consume the old handle and mint the successor on the same surface;
            // handles are never re-pointed in place.
            self.interactive_surface = self
                .interactive_surface
                .take()
                .map(|surface| surface.into_replaced(session_id));
            return Ok(());
        }
        let surface = self
            .client
            .attach_interactive_session(session_id, AttachSurfaceOptions::default())?;
        self.interactive_surface = Some(surface);
        Ok(())
    }

    fn surface_is_attached_to_session(
        &self,
        surface_id: &SurfaceId,
        session_id: &SessionId,
    ) -> bool {
        self.surface_session_id(surface_id).as_ref() == Some(session_id)
    }

    fn surface_session_id(&self, surface_id: &SurfaceId) -> Option<SessionId> {
        let routing = self
            .app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        routing.surface_session(surface_id).cloned()
    }

    pub fn start_passive_event_pump(
        &mut self,
        session_id: SessionId,
        event_tx: mpsc::Sender<CoreEvent>,
    ) -> Result<(), ClientError> {
        if self
            .event_pump_session_id
            .as_ref()
            .is_some_and(|active| active == &session_id)
        {
            return Ok(());
        }
        if let Some(handle) = self.event_pump.take() {
            handle.abort();
        }
        self.event_pump_session_id = None;
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        let mut connection = adapter.connect();
        let surface = connection
            .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
            .map_err(crate::local_client::client_error_from_attach)?;
        let surface_id = surface.surface_id;
        self.event_pump = Some(tokio::spawn(async move {
            while let Some(delivery) = connection.events_mut().recv().await {
                if delivery.surface_id == surface_id {
                    let Some(event) =
                        scope_surface_event(delivery.envelope.session_id, delivery.envelope.event)
                    else {
                        continue;
                    };
                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }));
        self.event_pump_session_id = Some(session_id);
        Ok(())
    }

    pub async fn bind_interactive_session(
        &mut self,
        session: crate::session_runtime::SessionHandle,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<(), ClientError> {
        let session_id = session.session_id().clone();
        self.register_session_runtime(session).await;
        self.ensure_interactive_surface(session_id.clone())?;
        if let Some(event_tx) = event_tx {
            self.start_passive_event_pump(session_id, event_tx)?;
        }
        Ok(())
    }

    pub fn activate_existing_interactive_session(
        &mut self,
        session_id: SessionId,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<(), ClientError> {
        self.ensure_interactive_surface(session_id.clone())?;
        if let Some(event_tx) = event_tx {
            self.start_passive_event_pump(session_id, event_tx)?;
        }
        Ok(())
    }

    pub async fn start_interactive_session(
        &mut self,
        params: coco_types::SessionStartParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let surface = self
            .client
            .session_start_handle(&self.handler, params)
            .await?;
        self.bind_lifecycle_surface(surface, event_tx)
    }

    pub async fn resume_interactive_session(
        &mut self,
        params: coco_types::SessionResumeParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let surface = self
            .client
            .session_resume_handle(&self.handler, params)
            .await?;
        self.bind_lifecycle_surface(surface, event_tx)
    }

    pub async fn replace_interactive_session_with_resume(
        &mut self,
        params: coco_types::SessionResumeParams,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let current = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        if current.session_id() == &params.target.session_id {
            return self.resume_interactive_session(params, event_tx).await;
        }
        let surface = self
            .client
            .replace_session_with_resume(&self.handler, current, params)
            .await
            .map_err(|(_session, error)| error)?;
        self.bind_lifecycle_surface(surface, event_tx)
    }

    pub async fn replace_interactive_session_with_clear(
        &mut self,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let current = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        let surface = self
            .client
            .replace_session_with_clear(&self.handler, current)
            .await
            .map_err(|(_session, error)| error)?;
        self.bind_lifecycle_surface(surface, event_tx)
    }

    fn bind_lifecycle_surface(
        &mut self,
        surface: crate::local_client::LocalSessionClient,
        event_tx: Option<mpsc::Sender<CoreEvent>>,
    ) -> Result<AppServerLocalSessionBinding, ClientError> {
        let session_id = surface.session_id().clone();
        let handle = self.app_server.registry().get(&session_id).ok_or_else(|| {
            ClientError::InvalidArgument(format!(
                "local lifecycle returned unregistered session {session_id}"
            ))
        })?;
        let session = handle.into_session();
        self.interactive_surface = Some(surface.clone());
        if let Some(event_tx) = event_tx {
            self.start_passive_event_pump(session_id, event_tx)?;
        }
        Ok(AppServerLocalSessionBinding { session, surface })
    }

    pub async fn drain_interactive_events_to(&mut self, event_tx: &mpsc::Sender<CoreEvent>) {
        let Some(surface) = self.interactive_surface.clone() else {
            return;
        };
        for pass in 0..2 {
            while let Some(envelope) = self.client.try_next_session_event(&surface) {
                let Some(event) = scope_surface_event(envelope.session_id, envelope.event) else {
                    continue;
                };
                if event_tx.send(event).await.is_err() {
                    return;
                }
            }
            if pass == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    pub async fn start_turn_and_wait_for_end(
        &mut self,
        session_id: SessionId,
        mut params: coco_types::TurnStartParams,
    ) -> Result<AppServerLocalTurnCompletion, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        params.target = surface.interactive_target();
        let handler = self.handler.clone();
        let started = self.client.turn_start(&handler, params).await?;
        loop {
            let Some(envelope) = self.client.next_session_event(&surface).await else {
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
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .as_ref()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        params.target = surface.interactive_target();
        self.client.turn_start(&self.handler, params).await
    }
}
