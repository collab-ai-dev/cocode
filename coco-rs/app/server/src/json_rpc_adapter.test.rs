use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use coco_app_server_transport::{JsonRpcNotification, JsonRpcSuccess, NdjsonDuplexConnection};
use coco_types::{
    ClientRequestMethod, ServerNotification, ServerRequest, ServerRequestUserInputParams,
    SessionEnvelope, SessionId, SessionState, SurfaceId, TurnId,
};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncWriteExt, BufReader, split};
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

use super::*;
use crate::{AppServer, AttachSurfaceOptions, SurfaceCapabilities, SurfaceCapability, SurfaceRole};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

#[derive(Default, Clone)]
struct RecordingHandler {
    methods: Arc<Mutex<Vec<ClientRequestMethod>>>,
}

#[derive(Default)]
struct BlockingHandler {
    slow_started: Arc<tokio::sync::Notify>,
    release_slow: Arc<tokio::sync::Notify>,
}

impl RecordingHandler {
    fn methods(&self) -> Vec<ClientRequestMethod> {
        self.methods.lock().expect("handler lock").clone()
    }
}

impl JsonRpcRequestHandler for RecordingHandler {
    fn handle_json_rpc_request(
        &self,
        _context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        self.methods
            .lock()
            .expect("handler lock")
            .push(request.method());
        Box::pin(async { Ok(serde_json::json!({ "ok": true })) })
    }
}

impl JsonRpcConnectionHandlerFactory for RecordingHandler {
    type Handler = Self;

    fn open(&self, _connection: ConnectionKey) -> Arc<Self::Handler> {
        Arc::new(self.clone())
    }
}

impl JsonRpcRequestHandler for BlockingHandler {
    fn handle_json_rpc_request(
        &self,
        _context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let slow_started = Arc::clone(&self.slow_started);
        let release_slow = Arc::clone(&self.release_slow);
        Box::pin(async move {
            if request.method() == ClientRequestMethod::SessionRead {
                slow_started.notify_one();
                release_slow.notified().await;
            }
            Ok(serde_json::json!({ "ok": true }))
        })
    }
}

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

#[test]
fn json_rpc_adapter_encodes_server_request_and_tracks_response_id() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let session_id = test_session_id("sess-1");
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection.connection_key(),
            surface_id.clone(),
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
        .expect("attach remote interactive surface");
    let routed = server
        .route_server_request(
            session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route server request");
    let delivery = connection
        .server_requests_mut()
        .try_recv()
        .expect("request delivery");

    let frame = connection
        .encode_server_request(delivery)
        .expect("encode server request");
    let JsonRpcFrame::Request(request) = frame else {
        panic!("expected request frame");
    };
    assert_eq!(
        request.id,
        json_rpc_id_from_request_id(&routed.pending.request_id)
    );
    assert_eq!(request.method, "input/requestUserInput");
    assert_eq!(
        request.params.as_ref().expect("request params")["request_id"],
        "payload-request-id"
    );

    let response = connection
        .complete_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "answer": "yes" }),
        )))
        .expect("complete response correlation");
    let JsonRpcServerRequestResponse::Success { pending, result } = response else {
        panic!("expected success response");
    };
    assert_eq!(pending.surface_id, surface_id);
    assert_eq!(pending.request_id, routed.pending.request_id);
    assert!(matches!(
        pending.request,
        ServerRequest::RequestUserInput(_)
    ));
    assert_eq!(result, serde_json::json!({ "answer": "yes" }));
}

#[test]
fn json_rpc_adapter_rejects_unknown_or_non_response_frames() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let mut connection = adapter.connect();

    assert!(matches!(
        connection.complete_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("missing".to_string()),
            serde_json::json!(true),
        ))),
        Err(JsonRpcAdapterError::UnknownResponseId { .. })
    ));
    assert!(matches!(
        connection.complete_server_request_response(JsonRpcFrame::Notification(
            JsonRpcNotification::new("session/event", None),
        )),
        Err(JsonRpcAdapterError::UnexpectedResponseFrame { .. })
    ));
}

#[tokio::test]
async fn json_rpc_adapter_dispatches_client_request_to_handler() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let handler = RecordingHandler::default();

    let response = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::String("req-1".to_string()),
                "turn/interrupt",
                Some(serde_json::json!({
                    "session_id": "session-a",
                    "surface_id": "surface-a",
                })),
            ),
            &handler,
        )
        .await;

    assert_eq!(handler.methods(), vec![ClientRequestMethod::TurnInterrupt]);
    assert_eq!(
        response,
        JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-1".to_string()),
            serde_json::json!({ "ok": true }),
        ))
    );
}

#[tokio::test]
async fn json_rpc_adapter_accepts_unit_request_with_empty_params() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let handler = RecordingHandler::default();

    let response = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::String("req-1".to_string()),
                "control/keepAlive",
                Some(serde_json::json!({})),
            ),
            &handler,
        )
        .await;

    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
    assert!(matches!(response, JsonRpcFrame::Success(_)));
}

#[test]
fn json_rpc_adapter_resolves_server_request_response_through_app_server() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let session_id = test_session_id("sess-1");
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection.connection_key(),
            surface_id.clone(),
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
        .expect("attach remote interactive surface");
    let routed = server
        .route_server_request(
            session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route server request");
    let delivery = connection
        .server_requests_mut()
        .try_recv()
        .expect("request delivery");
    let JsonRpcFrame::Request(request) = connection
        .encode_server_request(delivery)
        .expect("encode server request")
    else {
        panic!("expected request frame");
    };

    let resolved = connection
        .resolve_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "answer": "yes" }),
        )))
        .expect("resolve server request response");

    assert_eq!(resolved.pending, routed.pending);
    let ServerRequestReply::UserInput(params) = resolved.reply else {
        panic!("expected user input reply");
    };
    assert_eq!(params.request_id, "payload-request-id");
    assert_eq!(params.answer, "yes");
    let routing = server.routing().read().expect("routing lock");
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
}

#[tokio::test]
async fn json_rpc_owner_task_disconnects_app_server_on_transport_eof() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            test_session_id("sess-1"),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (server_read, server_write) = split(server_stream);
    let transport = NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
    drop(client_stream);

    let outcome = connection
        .run_ndjson_transport(transport, Arc::new(RecordingHandler::default()))
        .await
        .expect("owner loop exits cleanly");

    assert_eq!(outcome.detached_surfaces, vec![surface_id]);
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn json_rpc_owner_task_disconnects_app_server_on_transport_error() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            test_session_id("sess-1"),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (server_read, server_write) = split(server_stream);
    let transport = NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
    let owner = tokio::spawn(
        connection.run_ndjson_transport(transport, Arc::new(RecordingHandler::default())),
    );
    let (_client_read, mut client_write) = split(client_stream);
    client_write
        .write_all(b"not-json\n")
        .await
        .expect("write invalid frame");

    let error = owner
        .await
        .expect("owner task")
        .expect_err("invalid frame should fail owner");
    assert!(matches!(
        error,
        JsonRpcConnectionOwnerError::Transport { .. }
    ));
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn json_rpc_frame_channel_owner_dispatches_request_and_disconnects_on_eof() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            test_session_id("sess-1"),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(8);
    let handler = Arc::new(RecordingHandler::default());
    let owner =
        tokio::spawn(connection.run_frame_channels(inbound_rx, outbound_tx, Arc::clone(&handler)));

    inbound_tx
        .send(JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::Number(7),
            "control/keepAlive",
            Some(serde_json::json!({})),
        )))
        .await
        .expect("send inbound request");
    let response = outbound_rx.recv().await.expect("outbound response");
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
    assert_eq!(
        response,
        JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::Number(7),
            serde_json::json!({ "ok": true }),
        ))
    );

    drop(inbound_tx);
    let outcome = owner
        .await
        .expect("owner task")
        .expect("owner loop exits cleanly");
    assert_eq!(outcome.detached_surfaces, vec![surface_id]);
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn frame_channel_dispatch_does_not_block_fast_request_behind_slow_request() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(8);
    let handler = Arc::new(BlockingHandler::default());
    let owner =
        tokio::spawn(connection.run_frame_channels(inbound_rx, outbound_tx, Arc::clone(&handler)));

    inbound_tx
        .send(JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::Number(1),
            "session/read",
            Some(serde_json::json!({
                "target": { "session_id": "sess-slow" },
            })),
        )))
        .await
        .expect("send slow request");
    handler.slow_started.notified().await;
    inbound_tx
        .send(JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::Number(2),
            "control/keepAlive",
            Some(serde_json::json!({})),
        )))
        .await
        .expect("send fast request");

    let fast = tokio::time::timeout(Duration::from_millis(100), outbound_rx.recv())
        .await
        .expect("fast request must not wait for slow request")
        .expect("fast response");
    assert!(matches!(
        fast,
        JsonRpcFrame::Success(JsonRpcSuccess {
            id: JsonRpcId::Number(2),
            ..
        })
    ));

    handler.release_slow.notify_one();
    let slow = outbound_rx.recv().await.expect("slow response");
    assert!(matches!(
        slow,
        JsonRpcFrame::Success(JsonRpcSuccess {
            id: JsonRpcId::Number(1),
            ..
        })
    ));
    drop(inbound_tx);
    owner
        .await
        .expect("owner task")
        .expect("owner exits cleanly");
}

#[tokio::test]
async fn json_rpc_frame_channel_owner_disconnects_slow_outbound_consumer() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity_and_write_timeout(
        Arc::clone(&server),
        8,
        Duration::from_millis(10),
    );
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let session_id = test_session_id("sess-1");
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let (_inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(8);
    let (outbound_tx, _stalled_outbound_rx) = tokio::sync::mpsc::channel(1);
    let owner = tokio::spawn(connection.run_frame_channels(
        inbound_rx,
        outbound_tx,
        Arc::new(RecordingHandler::default()),
    ));

    assert_eq!(
        server
            .route_envelope(durable_envelope(session_id.clone(), 1))
            .delivered,
        1
    );
    assert_eq!(
        server
            .route_envelope(durable_envelope(session_id, 2))
            .delivered,
        1
    );

    let error = owner
        .await
        .expect("owner task")
        .expect_err("slow outbound consumer should fail");
    assert!(matches!(error, JsonRpcAdapterError::SlowConsumer { .. }));
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn json_rpc_frame_channel_owner_disconnects_after_adapter_error() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            test_session_id("sess-1"),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(8);
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(8);
    let owner = tokio::spawn(connection.run_frame_channels(
        inbound_rx,
        outbound_tx,
        Arc::new(RecordingHandler::default()),
    ));

    inbound_tx
        .send(JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("missing".to_string()),
            serde_json::json!({}),
        )))
        .await
        .expect("send unexpected response");
    let error = owner
        .await
        .expect("owner task")
        .expect_err("unexpected response should fail owner");
    assert!(matches!(
        error,
        JsonRpcAdapterError::UnknownResponseId { .. }
    ));
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn json_rpc_websocket_owner_dispatches_request_and_disconnects_on_close() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let connection_key = connection.connection_key();
    let surface_id = SurfaceId::from("surface-1");
    server
        .attach_surface_with_options(
            connection_key,
            surface_id.clone(),
            test_session_id("sess-1"),
            AttachSurfaceOptions::default(),
        )
        .expect("attach surface");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind websocket listener");
    let addr = listener.local_addr().expect("listener addr");
    let handler = Arc::new(RecordingHandler::default());
    let handler_for_owner = Arc::clone(&handler);
    let owner = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket tcp");
        let websocket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket");
        connection
            .run_websocket_transport(websocket, handler_for_owner)
            .await
    });

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .expect("connect websocket client");
    let request = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-ws".to_string()),
        "control/keepAlive",
        None,
    ));
    client
        .send(WebSocketMessage::Text(
            serde_json::to_string(&request)
                .expect("encode request")
                .into(),
        ))
        .await
        .expect("send websocket request");

    let message = client
        .next()
        .await
        .expect("websocket response")
        .expect("response message");
    let text = message.into_text().expect("text response");
    let frame: JsonRpcFrame = serde_json::from_str(text.as_ref()).expect("decode response");
    assert_eq!(
        frame,
        JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-ws".to_string()),
            serde_json::json!({ "ok": true }),
        ))
    );

    client.close(None).await.expect("close websocket");
    let outcome = owner
        .await
        .expect("owner task")
        .expect("owner exits cleanly");
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
    assert_eq!(outcome.detached_surfaces, vec![surface_id]);
    assert_eq!(
        server
            .routing()
            .read()
            .expect("routing lock")
            .connection_surface_count(connection_key),
        0
    );
}

#[tokio::test]
async fn json_rpc_adapter_websocket_listener_runs_until_shutdown() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind websocket listener");
    let addr = listener.local_addr().expect("listener addr");
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let handler = Arc::new(RecordingHandler::default());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener_task = tokio::spawn({
        let adapter = adapter.clone();
        let handler = Arc::clone(&handler);
        async move {
            adapter
                .run_websocket_listener_until_shutdown(listener, handler, shutdown_rx)
                .await
        }
    });

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .expect("connect websocket client");
    client
        .send(WebSocketMessage::Text(
            serde_json::to_string(&JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::String("req-ws-listener".to_string()),
                "control/keepAlive",
                None,
            )))
            .expect("encode request")
            .into(),
        ))
        .await
        .expect("send websocket request");

    let message = client
        .next()
        .await
        .expect("websocket response")
        .expect("response message");
    let text = message.into_text().expect("text response");
    let frame: JsonRpcFrame = serde_json::from_str(text.as_ref()).expect("decode response");
    assert_eq!(
        frame,
        JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-ws-listener".to_string()),
            serde_json::json!({ "ok": true }),
        ))
    );

    client.close(None).await.expect("close websocket");
    shutdown_tx.send(()).expect("send shutdown");
    listener_task
        .await
        .expect("listener task")
        .expect("listener exits cleanly");
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
}

#[cfg(unix)]
#[tokio::test]
async fn json_rpc_adapter_accepts_unix_connection_and_dispatches_requests() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
        .expect("bind unix listener");
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let handler = Arc::new(RecordingHandler::default());
    let (owner_task, client) = tokio::join!(
        adapter.accept_unix_connection(&listener, Arc::clone(&handler)),
        coco_app_server_transport::connect_ndjson_unix(&socket_path)
    );
    let owner_task = owner_task.expect("accept unix connection");
    let mut client = client.expect("connect unix socket");
    client
        .send_frame(&JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-uds".to_string()),
            "control/keepAlive",
            None,
        )))
        .await
        .expect("client sends request");

    let Some(JsonRpcFrame::Success(response)) =
        client.recv_frame().await.expect("client reads response")
    else {
        panic!("expected success response");
    };
    assert_eq!(response.id, JsonRpcId::String("req-uds".to_string()));
    assert_eq!(response.result, serde_json::json!({ "ok": true }));

    drop(client);
    owner_task
        .await
        .expect("owner task")
        .expect("owner exits cleanly");
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
}

#[cfg(unix)]
#[tokio::test]
async fn json_rpc_adapter_unix_listener_runs_until_shutdown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
        .expect("bind unix listener");
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let handler = Arc::new(RecordingHandler::default());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener_task = tokio::spawn({
        let adapter = adapter.clone();
        let handler = Arc::clone(&handler);
        async move {
            adapter
                .run_unix_listener_until_shutdown(listener, handler, shutdown_rx)
                .await
        }
    });

    let mut client = coco_app_server_transport::connect_ndjson_unix(&socket_path)
        .await
        .expect("connect unix socket");
    client
        .send_frame(&JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-listener".to_string()),
            "control/keepAlive",
            None,
        )))
        .await
        .expect("client sends request");

    let Some(JsonRpcFrame::Success(response)) =
        client.recv_frame().await.expect("client reads response")
    else {
        panic!("expected success response");
    };
    assert_eq!(response.id, JsonRpcId::String("req-listener".to_string()));
    assert_eq!(response.result, serde_json::json!({ "ok": true }));

    drop(client);
    shutdown_tx.send(()).expect("send shutdown");
    listener_task
        .await
        .expect("listener task")
        .expect("listener exits cleanly");
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
}

#[cfg(unix)]
#[tokio::test]
async fn json_rpc_adapter_bind_unix_listener_cleans_socket_on_shutdown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let handler = Arc::new(RecordingHandler::default());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener_task = tokio::spawn({
        let adapter = adapter.clone();
        let handler = Arc::clone(&handler);
        let socket_path = socket_path.clone();
        async move {
            adapter
                .bind_and_run_unix_listener_until_shutdown(socket_path, handler, shutdown_rx)
                .await
        }
    });

    let mut client = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            match coco_app_server_transport::connect_ndjson_unix(&socket_path).await {
                Ok(client) => break client,
                Err(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("listener starts");
    assert!(socket_path.exists(), "listener should create socket path");
    client
        .send_frame(&JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-bound-listener".to_string()),
            "control/keepAlive",
            None,
        )))
        .await
        .expect("client sends request");

    let Some(JsonRpcFrame::Success(response)) =
        client.recv_frame().await.expect("client reads response")
    else {
        panic!("expected success response");
    };
    assert_eq!(
        response.id,
        JsonRpcId::String("req-bound-listener".to_string())
    );
    assert_eq!(response.result, serde_json::json!({ "ok": true }));

    drop(client);
    shutdown_tx.send(()).expect("send shutdown");
    listener_task
        .await
        .expect("listener task")
        .expect("listener exits cleanly");
    assert!(
        !socket_path.exists(),
        "listener drop should remove socket path"
    );
    assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
}
