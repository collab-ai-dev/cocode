use std::sync::{Arc, Mutex};

use coco_app_server_transport::{JsonRpcFrame, JsonRpcRequest, JsonRpcSuccess};
use coco_types::{
    ClientRequestMethod, CoreEvent, RequestId, ServerNotification, ServerRequest,
    ServerRequestDelivery, ServerRequestUserInputParams, SessionDelivery, SessionEnvelope,
    SessionId, SessionLifecycleEffect, SessionLifecycleEffectKind, SessionState,
};

use super::*;
use crate::AppServer;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle;

fn session(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn envelope(session_id: SessionId) -> SessionEnvelope {
    SessionEnvelope::durable(
        session_id,
        None,
        None,
        1,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    )
}

fn server_request() -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: "payload-id".to_string(),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

#[derive(Default)]
struct RecordingHandler {
    calls: Arc<Mutex<Vec<(ConnectionKey, ClientRequestMethod)>>>,
}

impl JsonRpcRequestHandler for RecordingHandler {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: coco_types::ClientRequest,
    ) -> JsonRpcRequestFuture {
        self.calls
            .lock()
            .expect("calls")
            .push((context.connection, request.method()));
        Box::pin(async { Ok(serde_json::json!({ "ok": true })) })
    }
}

#[test]
fn session_event_wire_shape_is_keyed_by_session() {
    let session_id = session("session-event");
    let frame = encode_session_delivery(SessionDelivery {
        envelope: envelope(session_id.clone()),
    })
    .expect("encode");
    let JsonRpcFrame::Notification(notification) = frame else {
        panic!("expected notification");
    };
    let params = notification.params.expect("params");

    assert_eq!(notification.method, coco_types::SESSION_EVENT_METHOD);
    assert_eq!(params["envelope"]["session_id"], session_id.as_str());
    assert_eq!(params.as_object().expect("params object").len(), 1);
}

#[test]
fn lifecycle_wire_shape_is_keyed_by_session() {
    let session_id = session("session-lifecycle");
    let frame = encode_lifecycle_delivery(SessionLifecycleEffect {
        kind: SessionLifecycleEffectKind::SessionEnded {
            session_id: session_id.clone(),
        },
    });
    let JsonRpcFrame::Notification(notification) = frame else {
        panic!("expected notification");
    };
    let params = notification.params.expect("params");

    assert_eq!(notification.method, coco_types::SESSION_LIFECYCLE_METHOD);
    assert_eq!(params["effect"]["session_id"], session_id.as_str());
    assert_eq!(params.as_object().expect("params object").len(), 1);
}

#[test]
fn server_request_response_correlation_is_connection_local() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::new(server);
    let mut connection = adapter.connect();
    let request_id = RequestId::String("server-request-1".to_string());
    let frame = connection
        .encode_server_request(ServerRequestDelivery {
            session_id: session("session-request"),
            request_id: request_id.clone(),
            request: server_request(),
        })
        .expect("encode request");
    let JsonRpcFrame::Request(request) = frame else {
        panic!("expected request");
    };

    let response = connection
        .complete_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "answer": "yes" }),
        )))
        .expect("complete response");
    let JsonRpcServerRequestResponse::Success { pending, .. } = response else {
        panic!("expected success");
    };
    assert_eq!(pending.request_id, request_id);
}

#[test]
fn cancellation_is_a_notification_and_purges_response_correlation() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::new(server);
    let mut connection = adapter.connect();
    let request_id = RequestId::String("server-request-cancelled".to_string());
    let request_frame = connection
        .encode_server_request(ServerRequestDelivery {
            session_id: session("session-cancelled"),
            request_id: request_id.clone(),
            request: server_request(),
        })
        .expect("encode request");
    let JsonRpcFrame::Request(request) = request_frame else {
        panic!("expected request");
    };

    let cancel_frame = connection
        .encode_server_request(ServerRequestDelivery {
            session_id: session("session-cancelled"),
            request_id,
            request: ServerRequest::CancelRequest(coco_types::ServerCancelRequestParams {
                request_id: "server-request-cancelled".to_string(),
                reason: Some("peer answered".to_string()),
            }),
        })
        .expect("encode cancellation");

    assert!(matches!(cancel_frame, JsonRpcFrame::Notification(_)));
    assert!(
        connection
            .complete_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "answer": "late" }),
            )))
            .is_err()
    );
}

#[tokio::test]
async fn request_dispatch_preserves_the_registered_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::new(server);
    let connection = adapter.connect();
    let key = connection.connection_key();
    let handler = RecordingHandler::default();
    let request = JsonRpcRequest::new(
        coco_app_server_transport::JsonRpcId::Number(1),
        ClientRequestMethod::KeepAlive.as_str(),
        None,
    );

    let response = connection.dispatch_client_request(request, &handler).await;

    assert!(matches!(response, JsonRpcFrame::Success(_)));
    assert_eq!(
        handler.calls.lock().expect("calls").as_slice(),
        &[(key, ClientRequestMethod::KeepAlive)]
    );
}

#[test]
fn dropping_json_rpc_connection_disconnects_all_attachments() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::new(Arc::clone(&server));
    let connection = adapter.connect();
    let key = connection.connection_key();
    let session_id = session("session-drop");
    assert!(matches!(
        server.registry().begin_load(session_id.clone()),
        Ok(crate::LoadStart::Reserved)
    ));
    server
        .registry()
        .complete_load_success(&session_id, TestHandle)
        .expect("complete live session");
    server
        .attach_live_session(key, session_id, crate::AttachSessionOptions::full())
        .expect("attach live session");
    drop(connection);

    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing")
            .connection_session_count(key),
        0
    );
}
