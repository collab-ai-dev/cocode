use std::sync::Arc;

use coco_app_server::{AttachSurfaceOptions, LocalClientAdapter};
use coco_app_server_client::ClientError;
use coco_types::{CoreEvent, ServerNotification, SessionId, SurfaceId};
use tokio::sync::mpsc;

use super::{AppServerLocalBridge, AppServerLocalTurnCompletion, AppSessionHandle};

impl AppServerLocalBridge {
    pub fn interactive_session(&self) -> Option<&crate::local_client::LocalSessionClient> {
        self.interactive_surface.as_ref()
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
                if delivery.surface_id == surface_id
                    && event_tx.send(delivery.envelope.event).await.is_err()
                {
                    break;
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

    pub async fn drain_interactive_events_to(&mut self, event_tx: &mpsc::Sender<CoreEvent>) {
        let Some(surface) = self.interactive_surface.clone() else {
            return;
        };
        for pass in 0..2 {
            while let Some(envelope) = self.client.try_next_session_event(&surface) {
                if event_tx.send(envelope.event).await.is_err() {
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
                return Ok(AppServerLocalTurnCompletion { started, ended });
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

    pub async fn current_session_result(&self) -> Option<coco_types::SessionResultParams> {
        let session_id = if let Some(surface) = self.interactive_surface.as_ref()
            && let Some(session_id) = self.surface_session_id(surface.surface_id())
        {
            session_id
        } else {
            return None;
        };
        let runtime = self
            .app_server
            .registry()
            .get(&session_id)
            .map(AppSessionHandle::into_session)?;
        Some(crate::session_archive::build_session_result(
            &runtime, "end_turn",
        ))
    }
}
