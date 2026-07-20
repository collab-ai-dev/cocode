use std::sync::{Arc, Mutex};

use coco_types::{
    ClientRequest, CoreEvent, ServerNotification, ServerRequest, ServerRequestUserInputParams,
    SessionEnvelope, SessionId, SessionState,
};

use super::*;
use crate::{AppServer, AttachSessionOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle;

fn session(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn activate(server: &AppServer<TestHandle>, session_id: &SessionId) {
    assert!(matches!(
        server.registry().begin_load(session_id.clone()),
        Ok(crate::LoadStart::Reserved)
    ));
    server
        .registry()
        .complete_load_success(session_id, TestHandle)
        .expect("activate test session");
}

fn envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
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
    calls: Arc<Mutex<Vec<(ConnectionKey, ClientRequest)>>>,
}

impl LocalClientRequestHandler for RecordingHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            calls
                .lock()
                .expect("calls")
                .push((context.connection_key(), request));
            Ok(serde_json::json!({ "ok": true }))
        })
    }
}

#[tokio::test]
async fn cloned_handle_dispatches_on_the_same_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(server);
    let connection = adapter.connect();
    let handle = connection.handle();
    let cloned = handle.clone();
    let handler = RecordingHandler::default();

    cloned
        .dispatch_client_request(&handler, ClientRequest::KeepAlive)
        .await
        .expect("dispatch");

    let calls = handler.calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, connection.connection_key());
    assert_eq!(cloned.connection_key(), connection.connection_key());
}

#[tokio::test]
async fn one_local_connection_receives_events_for_multiple_attached_sessions() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let mut connection = adapter.connect();
    let first = session("session-first");
    let second = session("session-second");
    activate(&server, &first);
    activate(&server, &second);
    connection
        .handle()
        .attach_session(first.clone(), AttachSessionOptions::full())
        .expect("attach first");
    connection
        .handle()
        .attach_session(second.clone(), AttachSessionOptions::full())
        .expect("attach second");

    server.route_envelope(envelope(first.clone(), 1));
    server.route_envelope(envelope(second.clone(), 1));

    let LocalClientInbound::Event(first_delivery) = connection.recv().await.expect("first") else {
        panic!("expected event");
    };
    let LocalClientInbound::Event(second_delivery) = connection.recv().await.expect("second")
    else {
        panic!("expected event");
    };
    let delivered = [
        first_delivery.envelope.session_id.clone(),
        second_delivery.envelope.session_id.clone(),
    ];
    assert!(delivered.contains(&first));
    assert!(delivered.contains(&second));
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing")
            .connection_session_count(connection.connection_key()),
        2
    );
}

#[tokio::test]
async fn unified_receive_path_drains_server_requests() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let mut connection = adapter.connect();
    let session_id = session("session-request");
    activate(&server, &session_id);
    connection
        .handle()
        .attach_session(session_id.clone(), AttachSessionOptions::full())
        .expect("attach request session");
    let routed = server
        .routing()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .route_server_request(session_id, None, server_request())
        .expect("route");

    let LocalClientInbound::ServerRequest(delivery) = connection.recv().await.expect("request")
    else {
        panic!("expected server request");
    };
    assert_eq!(delivery.request_id, routed.pending.request_id);
}

#[test]
fn dropping_the_connection_detaches_all_sessions_but_cloned_handles_do_not() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let connection = adapter.connect();
    let key = connection.connection_key();
    let handle = connection.handle();
    let first = session("session-one");
    let second = session("session-two");
    activate(&server, &first);
    activate(&server, &second);
    handle
        .attach_session(first, AttachSessionOptions::full())
        .expect("attach first");
    handle
        .attach_session(second, AttachSessionOptions::full())
        .expect("attach second");
    let cloned_handle = handle.clone();
    assert_eq!(cloned_handle.connection_key(), key);
    drop(cloned_handle);
    assert_eq!(handle.connection_key(), key);
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing")
            .connection_session_count(key),
        2
    );

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
