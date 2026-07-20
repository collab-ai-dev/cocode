use std::sync::{Arc, Mutex};

use coco_app_server::{
    AppServer, AttachSessionOptions, LocalClientAdapter, LocalClientDispatchError,
    LocalClientRequestContext, LocalClientRequestFuture, LocalClientRequestHandler,
};
use coco_types::{
    ClientRequest, CoreEvent, ServerNotification, SessionEnvelope, SessionId, SessionState,
};

use super::*;

#[derive(Debug, Clone)]
struct TestHandle;

fn session(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn activate(server: &AppServer<TestHandle>, session_id: &SessionId) {
    assert!(matches!(
        server.registry().begin_load(session_id.clone()),
        Ok(coco_app_server::LoadStart::Reserved)
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

#[derive(Default)]
struct RecordingHandler {
    calls: Arc<Mutex<Vec<(coco_app_server::ConnectionKey, ClientRequest)>>>,
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
            Ok(serde_json::Value::Null)
        })
    }
}

#[tokio::test]
async fn cloning_a_local_client_creates_no_additional_app_server_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let client = LocalServerClient::connect_local(&adapter);
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );

    let first = client.clone();
    let second = client.clone();

    assert_eq!(first.connection_key(), client.connection_key());
    assert_eq!(second.connection_key(), client.connection_key());
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );
}

#[tokio::test]
async fn in_memory_observers_do_not_change_the_session_grant() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let client = LocalServerClient::connect_local(&adapter);
    let session_id = session("session-observed");
    activate(&server, &session_id);
    client
        .attach_full_session(session_id.clone())
        .expect("attach observed session");
    let mut observer = client.clone();

    let observed = observer.observe_session(session_id.clone());

    assert_eq!(observed.session_id(), &session_id);
    assert_eq!(observer.connection_key(), client.connection_key());
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing")
            .connection_counts_for_session(&session_id),
        coco_app_server::SessionConnectionCounts {
            full: 1,
            read_only: 0,
        }
    );
}

#[tokio::test]
async fn in_memory_observers_each_receive_the_same_session_event() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let client = LocalServerClient::connect_local(&adapter);
    let session_id = session("session-observer-fanout");
    activate(&server, &session_id);
    client
        .attach_full_session(session_id.clone())
        .expect("attach session");
    let mut first = client.clone();
    let first_session = first.observe_session(session_id.clone());
    let mut second = client.clone();
    let second_session = second.observe_session(session_id.clone());

    assert_eq!(
        server
            .route_envelope(envelope(session_id.clone(), 1))
            .delivered,
        1,
        "one physical connection receives one delivery"
    );
    let first_event = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        first.next_session_event(&first_session),
    )
    .await
    .expect("first observer stalled")
    .expect("first observer closed");
    let second_event = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        second.next_session_event(&second_session),
    )
    .await
    .expect("second observer stalled")
    .expect("second observer closed");

    assert_eq!(first_event.session_seq, Some(1));
    assert_eq!(second_event.session_seq, Some(1));
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );
}

#[tokio::test]
async fn small_physical_queue_handles_two_consecutive_event_bursts() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 1);
    let client = LocalServerClient::connect_local(&adapter);
    let session_id = session("session-two-turns");
    activate(&server, &session_id);
    client
        .attach_full_session(session_id.clone())
        .expect("attach session");
    let mut observer = client.clone();
    let observed = observer.observe_session(session_id.clone());

    for seq in 1..=4 {
        let outcome = server.route_envelope(envelope(session_id.clone(), seq));
        assert_eq!(
            outcome.delivered, 1,
            "event {seq} must reach the one connection"
        );
        assert!(outcome.disconnected.is_empty());
        tokio::task::yield_now().await;
    }

    for expected in 1..=4 {
        let received = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            observer.next_session_event(&observed),
        )
        .await
        .expect("observer did not stall")
        .expect("connection open");
        assert_eq!(received.session_seq, Some(expected));
    }
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );
}

#[tokio::test]
async fn command_clones_dispatch_with_the_original_connection_key() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(server);
    let client = LocalServerClient::connect_local(&adapter);
    let clone = client.clone();
    let handler = RecordingHandler::default();

    clone.keep_alive(&handler).await.expect("keep alive");

    let calls = handler.calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, client.connection_key());
    assert!(matches!(calls[0].1, ClientRequest::KeepAlive));
}

#[tokio::test]
async fn drop_of_last_client_owner_disconnects_the_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let client = LocalServerClient::connect_local(&adapter);
    let clone = client.clone();
    drop(client);
    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        1
    );

    drop(clone);
    tokio::task::yield_now().await;

    assert_eq!(
        server.routing().read().expect("routing").connection_count(),
        0
    );
}

#[test]
fn dispatch_error_conversion_preserves_server_fields() {
    let converted = dispatch_error(LocalClientDispatchError {
        code: 42,
        message: "denied".to_string(),
        data: Some(serde_json::json!({ "kind": "test" })),
    });
    assert!(matches!(
        converted,
        ClientError::Server { code: 42, message, .. } if message == "denied"
    ));
}

#[test]
fn full_attach_requires_a_live_session() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::new(Arc::clone(&server));
    let connection = adapter.connect();
    let session_id = session("session-direct");
    let error = connection
        .handle()
        .attach_session(session_id.clone(), AttachSessionOptions::full())
        .expect_err("ghost attachment must be rejected");
    assert!(matches!(
        error,
        coco_app_server::AttachError::SessionNotFound { .. }
    ));
    assert!(
        server
            .routing()
            .read()
            .expect("routing")
            .grant(connection.connection_key(), &session_id)
            .is_none()
    );
}
