//! Typed AppServer client handles.
//!
//! This Phase A slice provides the local in-process shape of the two-level
//! client contract. Remote transports and runtime-driving methods land later.

use std::collections::HashMap;
use std::collections::VecDeque;

use coco_app_server::AttachError;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::DetachSurfaceOutcome;
use coco_app_server::DisconnectOutcome;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientConnection;
use coco_app_server::LocalClientSubscribeOutcome;
use coco_app_server::ServerRequestDelivery;
use coco_app_server::SessionSurfaceCounts;
use coco_app_server::SurfaceDelivery;
use coco_app_server::SurfaceLifecycleDelivery;
use coco_app_server::SurfaceRole;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;

pub struct ServerClient<H> {
    connection: LocalClientConnection<H>,
    event_buffers: HashMap<SurfaceId, VecDeque<SessionEnvelope>>,
    request_buffers: HashMap<SurfaceId, VecDeque<ServerRequestDelivery>>,
    lifecycle_buffers: HashMap<SurfaceId, VecDeque<SurfaceLifecycleDelivery>>,
}

impl<H: Clone> ServerClient<H> {
    pub fn connect_local(adapter: &LocalClientAdapter<H>) -> Self {
        Self {
            connection: adapter.connect(),
            event_buffers: HashMap::new(),
            request_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
        }
    }

    pub fn attach_interactive_session(
        &self,
        session_id: SessionId,
        mut options: AttachSurfaceOptions,
    ) -> Result<SessionClient, ClientError> {
        options.role = SurfaceRole::Interactive;
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(ClientError::from)?;
        Ok(SessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
        })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<PassiveSessionClient, ClientError> {
        let subscription = self
            .connection
            .subscribe_surface(session_id, after_seq, options)
            .map_err(ClientError::from)?;
        match subscription {
            LocalClientSubscribeOutcome::Attached(subscription) => Ok(PassiveSessionClient {
                session_id: subscription.session_id,
                surface_id: subscription.surface_id,
                replayed: subscription.replayed,
            }),
            LocalClientSubscribeOutcome::SnapshotRequired => Err(ClientError::SnapshotRequired),
        }
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceDelivery> {
        self.connection.events_mut()
    }

    pub fn try_next_session_event(&mut self, session: &SessionClient) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub fn try_next_passive_event(
        &mut self,
        session: &PassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        self.connection.server_requests_mut()
    }

    pub fn try_next_session_request(
        &mut self,
        session: &SessionClient,
    ) -> Option<ServerRequestDelivery> {
        self.try_next_request_for_surface(session.surface_id())
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery> {
        self.connection.lifecycle_mut()
    }

    pub fn try_next_session_lifecycle(
        &mut self,
        session: &SessionClient,
    ) -> Option<SurfaceLifecycleDelivery> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn try_next_passive_lifecycle(
        &mut self,
        session: &PassiveSessionClient,
    ) -> Option<SurfaceLifecycleDelivery> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn detach_passive(
        &self,
        passive: PassiveSessionClient,
    ) -> Result<DetachSurfaceOutcome, (PassiveSessionClient, ClientError)> {
        let outcome = self.connection.detach_surface(&passive.surface_id);
        if outcome.detached_surface.is_some() {
            Ok(outcome)
        } else {
            Err((
                passive,
                ClientError::InvalidArgument("passive surface is not attached".to_string()),
            ))
        }
    }

    pub fn close(self) -> Result<DisconnectOutcome, ClientError> {
        Ok(self.connection.disconnect())
    }

    pub fn list_live_sessions(&self) -> Vec<LiveSessionSummary> {
        self.connection
            .list_live_sessions()
            .into_iter()
            .map(|summary| LiveSessionSummary {
                session_id: summary.session_id,
                surface_counts: summary.surface_counts,
            })
            .collect()
    }

    fn try_next_event_for_surface(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            let delivery = self.connection.events_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery.envelope);
            }
            self.event_buffers
                .entry(delivery.surface_id)
                .or_default()
                .push_back(delivery.envelope);
        }
    }

    fn pop_buffered_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        let queue = self.event_buffers.get_mut(surface_id)?;
        let envelope = queue.pop_front();
        if queue.is_empty() {
            self.event_buffers.remove(surface_id);
        }
        envelope
    }

    fn try_next_request_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<ServerRequestDelivery> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.request_buffers, surface_id) {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.server_requests_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.request_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn try_next_lifecycle_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleDelivery> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.lifecycle_buffers, surface_id)
        {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.lifecycle_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.lifecycle_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn pop_buffered_delivery<T>(
        buffers: &mut HashMap<SurfaceId, VecDeque<T>>,
        surface_id: &SurfaceId,
    ) -> Option<T> {
        let queue = buffers.get_mut(surface_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            buffers.remove(surface_id);
        }
        delivery
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
}

impl SessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }
}

#[derive(Debug, Clone)]
pub struct PassiveSessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
    replayed: Vec<SessionEnvelope>,
}

impl PassiveSessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn replayed(&self) -> &[SessionEnvelope] {
        &self.replayed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSessionSummary {
    pub session_id: SessionId,
    pub surface_counts: SessionSurfaceCounts,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport disconnected")]
    Disconnected,
    #[error("client invalid (reconnect and resume)")]
    ClientInvalid,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("snapshot required before subscribing")]
    SnapshotRequired,
}

impl From<AttachError> for ClientError {
    fn from(error: AttachError) -> Self {
        Self::InvalidArgument(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use coco_app_server::AppServer;
    use coco_app_server::LocalClientAdapter;
    use coco_app_server::SurfaceCapabilities;
    use coco_app_server::SurfaceCapability;
    use coco_app_server::SurfaceLifecycleEffect;
    use coco_app_server::SurfaceLifecycleEffectKind;
    use coco_types::CoreEvent;
    use coco_types::ServerNotification;
    use coco_types::ServerRequest;
    use coco_types::ServerRequestUserInputParams;
    use coco_types::SessionEnvelope;
    use coco_types::SessionState;
    use coco_types::TurnId;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestHandle(&'static str);

    fn test_session_id(value: &str) -> SessionId {
        SessionId::try_new(value).expect("valid test session id")
    }

    fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
        SessionEnvelope::durable(
            session_id,
            None,
            None,
            seq,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            }),
        )
    }

    fn test_server_request(label: &str) -> ServerRequest {
        ServerRequest::RequestUserInput(ServerRequestUserInputParams {
            request_id: format!("payload-request-{label}"),
            prompt: "continue?".to_string(),
            description: None,
            choices: Vec::new(),
            default: None,
        })
    }

    #[test]
    fn local_server_client_attaches_interactive_and_passive_surfaces() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        server.route_envelope(durable_envelope(session_id.clone(), 1));
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);

        let interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        assert_eq!(interactive.session_id(), &session_id);
        assert_eq!(passive.session_id(), &session_id);
        assert_eq!(passive.replayed().len(), 1);
        assert_eq!(
            server.list_live_sessions()[0].surface_counts,
            SessionSurfaceCounts {
                attached: 2,
                closed: 0,
            }
        );
        let outcome = server.route_envelope(durable_envelope(session_id, 2));
        assert_eq!(outcome.delivered, 2);
        assert_eq!(
            client
                .events_mut()
                .try_recv()
                .expect("first surface event")
                .envelope
                .session_seq,
            Some(2)
        );
        assert_eq!(
            client
                .events_mut()
                .try_recv()
                .expect("second surface event")
                .envelope
                .session_seq,
            Some(2)
        );
    }

    #[test]
    fn detach_passive_consumes_only_that_surface() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let _interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id, Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        let detached = client.detach_passive(passive).expect("detach passive");

        assert!(detached.detached_surface.is_some());
        assert_eq!(server.list_live_sessions()[0].surface_counts.attached, 1);
    }

    #[test]
    fn client_lists_live_sessions_with_surface_counts() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let _interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        assert_eq!(
            client.list_live_sessions(),
            vec![LiveSessionSummary {
                session_id: session_id.clone(),
                surface_counts: SessionSurfaceCounts {
                    attached: 2,
                    closed: 0,
                },
            }]
        );

        client.detach_passive(passive).expect("detach passive");

        assert_eq!(
            client.list_live_sessions(),
            vec![LiveSessionSummary {
                session_id,
                surface_counts: SessionSurfaceCounts {
                    attached: 1,
                    closed: 0,
                },
            }]
        );
    }

    #[test]
    fn session_event_demux_buffers_other_surfaces_on_same_connection() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let interactive_session_id = test_session_id("sess-interactive");
        let passive_session_id = test_session_id("sess-passive");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let interactive = client
            .attach_interactive_session(
                interactive_session_id.clone(),
                AttachSurfaceOptions::default(),
            )
            .expect("attach interactive");
        let passive = client
            .subscribe_session(
                passive_session_id.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe passive");

        server.route_envelope(durable_envelope(passive_session_id.clone(), 1));
        server.route_envelope(durable_envelope(interactive_session_id.clone(), 1));

        let interactive_event = client
            .try_next_session_event(&interactive)
            .expect("interactive event");
        let passive_event = client
            .try_next_passive_event(&passive)
            .expect("passive event");

        assert_eq!(interactive_event.session_id, interactive_session_id);
        assert_eq!(passive_event.session_id, passive_session_id);
        assert!(client.try_next_session_event(&interactive).is_none());
        assert!(client.try_next_passive_event(&passive).is_none());
    }

    #[test]
    fn session_request_demux_buffers_other_interactive_surfaces() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let first_session_id = test_session_id("sess-first");
        let second_session_id = test_session_id("sess-second");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let first = client
            .attach_interactive_session(
                first_session_id.clone(),
                AttachSurfaceOptions {
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach first interactive");
        let second = client
            .attach_interactive_session(
                second_session_id.clone(),
                AttachSurfaceOptions {
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach second interactive");

        let first_route = server
            .route_server_request(
                first_session_id.clone(),
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-first")),
                test_server_request("first"),
            )
            .expect("route first request");
        let second_route = server
            .route_server_request(
                second_session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-second")),
                test_server_request("second"),
            )
            .expect("route second request");

        let second_delivery = client
            .try_next_session_request(&second)
            .expect("second request");
        let first_delivery = client
            .try_next_session_request(&first)
            .expect("first request");

        assert_eq!(second_delivery.request_id, second_route.pending.request_id);
        assert_eq!(first_delivery.request_id, first_route.pending.request_id);
        assert!(client.try_next_session_request(&first).is_none());
        assert!(client.try_next_session_request(&second).is_none());
    }

    #[test]
    fn lifecycle_demux_buffers_other_surfaces_on_same_connection() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let interactive_session_id = test_session_id("sess-interactive");
        let passive_session_id = test_session_id("sess-passive");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let interactive = client
            .attach_interactive_session(
                interactive_session_id.clone(),
                AttachSurfaceOptions::default(),
            )
            .expect("attach interactive");
        let passive = client
            .subscribe_session(
                passive_session_id.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe passive");

        let outcome = server.route_lifecycle_effects(vec![
            SurfaceLifecycleEffect {
                surface_id: passive.surface_id().clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: passive_session_id.clone(),
                },
            },
            SurfaceLifecycleEffect {
                surface_id: interactive.surface_id().clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: interactive_session_id.clone(),
                },
            },
        ]);
        assert_eq!(outcome.delivered, 2);

        let interactive_delivery = client
            .try_next_session_lifecycle(&interactive)
            .expect("interactive lifecycle");
        let passive_delivery = client
            .try_next_passive_lifecycle(&passive)
            .expect("passive lifecycle");

        assert_eq!(
            interactive_delivery.surface_id,
            interactive.surface_id().clone()
        );
        assert_eq!(passive_delivery.surface_id, passive.surface_id().clone());
        assert!(client.try_next_session_lifecycle(&interactive).is_none());
        assert!(client.try_next_passive_lifecycle(&passive).is_none());
    }
}
