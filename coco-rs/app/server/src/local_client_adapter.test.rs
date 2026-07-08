use std::sync::Arc;
use std::sync::Mutex;

use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::ServerRequest;
use coco_types::ServerRequestUserInputParams;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionState;
use coco_types::TurnId;

use super::*;
use crate::AppServer;
use crate::AttachSurfaceOptions;
use crate::ServerRequestReply;
use crate::SurfaceCapabilities;
use crate::SurfaceCapability;
use crate::SurfaceLifecycleEffect;
use crate::SurfaceLifecycleEffectKind;
use crate::SurfaceRole;

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

fn test_server_request() -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: "payload-request-id".to_string(),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

#[derive(Default)]
struct RecordingRequestHandler {
    calls: Arc<Mutex<Vec<(ConnectionKey, ClientRequest)>>>,
    error: Option<LocalClientDispatchError>,
}

impl LocalClientRequestHandler for RecordingRequestHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let calls = Arc::clone(&self.calls);
        let error = self.error.clone();
        Box::pin(async move {
            calls
                .lock()
                .expect("calls lock")
                .push((context.connection_key(), request));
            match error {
                Some(error) => Err(error),
                None => Ok(serde_json::json!({ "ok": true })),
            }
        })
    }
}

#[tokio::test]
async fn local_adapter_dispatches_client_requests_to_handler() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let handler = RecordingRequestHandler::default();

    let result = connection
        .dispatch_client_request(&handler, ClientRequest::KeepAlive)
        .await
        .expect("dispatch succeeds");

    assert_eq!(result, serde_json::json!({ "ok": true }));
    let calls = handler.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, connection.connection_key());
    assert!(matches!(calls[0].1, ClientRequest::KeepAlive));
}

#[tokio::test]
async fn local_adapter_propagates_client_request_errors() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let handler = RecordingRequestHandler {
        error: Some(LocalClientDispatchError::invalid_params(
            "bad local request",
        )),
        ..RecordingRequestHandler::default()
    };

    let error = connection
        .dispatch_client_request(&handler, ClientRequest::KeepAlive)
        .await
        .expect_err("dispatch fails");

    assert_eq!(error.message, "bad local request");
}

#[test]
fn local_adapter_subscribes_with_replay_then_receives_live_events() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    server.route_envelope(durable_envelope(session_id.clone(), 1));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();

    let subscription = connection
        .subscribe_surface(session_id.clone(), Some(0), AttachSurfaceOptions::default())
        .expect("subscribe");

    let LocalClientSubscribeOutcome::Attached(subscription) = subscription else {
        panic!("expected attached subscription");
    };
    assert_eq!(subscription.replayed.len(), 1);
    assert_eq!(subscription.replayed[0].session_seq, Some(1));
    let outcome = server.route_envelope(durable_envelope(session_id, 2));
    assert_eq!(outcome.delivered, 1);
    let delivered = connection.events_mut().try_recv().expect("live delivery");
    assert_eq!(delivered.surface_id, subscription.surface_id);
    assert_eq!(delivered.envelope.session_seq, Some(2));
}

#[test]
fn local_adapter_registers_request_and_lifecycle_channels() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let surface = connection
        .attach_surface(
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let routed = server
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route request");
    let request_delivery = connection
        .server_requests_mut()
        .try_recv()
        .expect("request delivery");
    assert_eq!(request_delivery.surface_id, surface.surface_id);
    assert_eq!(request_delivery.request_id, routed.pending.request_id);

    let lifecycle_outcome = server.route_lifecycle_effects(vec![SurfaceLifecycleEffect {
        surface_id: surface.surface_id.clone(),
        kind: SurfaceLifecycleEffectKind::SessionStarted {
            session_id: session_id.clone(),
        },
    }]);
    assert_eq!(lifecycle_outcome.delivered, 1);
    let lifecycle_delivery = connection
        .lifecycle_mut()
        .try_recv()
        .expect("lifecycle delivery");
    assert_eq!(lifecycle_delivery.surface_id, surface.surface_id);
    assert_eq!(
        lifecycle_delivery.effect.kind,
        SurfaceLifecycleEffectKind::SessionStarted { session_id }
    );

    let reply = ServerRequestReply::UserInput(coco_types::UserInputResolveParams {
        request_id: request_delivery.request_id.as_display(),
        answer: "yes".to_string(),
    });
    let resolved = server
        .resolve_server_request(&surface.session_id, reply)
        .expect("resolve request");
    assert_eq!(resolved.pending, routed.pending);
}

#[test]
fn local_adapter_detaches_one_surface_without_closing_connection() {
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
    let mut connection = adapter.connect();
    let first = connection
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach first");
    let second = connection
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach second");

    let initial = server.route_envelope(durable_envelope(session_id.clone(), 1));
    assert_eq!(initial.delivered, 2);
    let mut delivered = vec![
        connection
            .events_mut()
            .try_recv()
            .expect("first initial delivery")
            .surface_id,
        connection
            .events_mut()
            .try_recv()
            .expect("second initial delivery")
            .surface_id,
    ];
    delivered.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let mut expected = vec![first.surface_id.clone(), second.surface_id.clone()];
    expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(delivered, expected);

    let detached = connection.detach_surface(&first.surface_id);

    assert_eq!(detached.detached_surface, Some(first.surface_id));
    assert!(detached.cancelled_requests.is_empty());
    let summaries = server.list_live_sessions();
    assert_eq!(summaries[0].surface_counts.attached, 1);
    let after_detach = server.route_envelope(durable_envelope(session_id, 2));
    assert_eq!(after_detach.delivered, 1);
    let delivered = connection
        .events_mut()
        .try_recv()
        .expect("remaining surface delivery");
    assert_eq!(delivered.surface_id, second.surface_id);
    assert_eq!(delivered.envelope.session_seq, Some(2));
}

#[test]
fn local_adapter_cannot_detach_surface_from_another_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut owner = adapter.connect();
    let other = adapter.connect();
    let surface = owner
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach surface");

    let denied = other.detach_surface(&surface.surface_id);

    assert_eq!(denied, DetachSurfaceOutcome::default());
    let outcome = server.route_envelope(durable_envelope(session_id, 1));
    assert_eq!(outcome.delivered, 1);
    let delivered = owner.events_mut().try_recv().expect("owner delivery");
    assert_eq!(delivered.surface_id, surface.surface_id);
}
