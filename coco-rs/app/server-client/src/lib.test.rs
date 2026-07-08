use std::sync::Arc;
use std::sync::Mutex;

use coco_app_server::AppServer;
use coco_app_server::ConnectionKey;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientDispatchError;
use coco_app_server::LocalClientRequestContext;
use coco_app_server::LocalClientRequestFuture;
use coco_app_server::LocalClientRequestHandler;
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
use tokio::io::BufReader;
use tokio::io::split;

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

struct RecordingLocalRequestHandler {
    calls: Arc<Mutex<Vec<(ConnectionKey, String)>>>,
    result: serde_json::Value,
    error: Option<LocalClientDispatchError>,
}

impl Default for RecordingLocalRequestHandler {
    fn default() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: serde_json::Value::Null,
            error: None,
        }
    }
}

impl LocalClientRequestHandler for RecordingLocalRequestHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let calls = Arc::clone(&self.calls);
        let result = self.result.clone();
        let error = self.error.clone();
        Box::pin(async move {
            calls.lock().expect("calls lock").push((
                context.connection_key(),
                request.method().as_str().to_string(),
            ));
            match error {
                Some(error) => Err(error),
                None => Ok(result),
            }
        })
    }
}

#[tokio::test]
async fn local_server_client_typed_methods_dispatch_and_decode_results() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = ServerClient::connect_local(&adapter);
    let session_id = test_session_id("sess-local-typed-client");
    let handler = RecordingLocalRequestHandler {
        result: serde_json::json!({ "session_id": session_id }),
        ..RecordingLocalRequestHandler::default()
    };

    let result = client
        .session_start(&handler, SessionStartParams::default())
        .await
        .expect("session start succeeds");

    assert_eq!(
        result.session_id,
        test_session_id("sess-local-typed-client")
    );
    {
        let calls = handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "session/start");
    }

    let unit_handler = RecordingLocalRequestHandler::default();
    client
        .user_input_resolve(
            &unit_handler,
            UserInputResolveParams {
                request_id: "input-1".to_string(),
                answer: "yes".to_string(),
            },
        )
        .await
        .expect("user input resolve succeeds");
    {
        let calls = unit_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "input/resolveUserInput");
    }

    let usage = coco_types::SessionUsageSnapshot::empty(test_session_id("sess-local-typed-client"));
    let cost_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(SessionCostResult {
            text: "No usage yet.".to_string(),
            usage,
        })
        .expect("cost result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let cost = client
        .session_cost(&cost_handler)
        .await
        .expect("session cost succeeds");
    assert_eq!(cost.text, "No usage yet.");
    {
        let calls = cost_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "session/cost");
    }

    let task_list_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(TaskListResult { tasks: Vec::new() })
            .expect("task list result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let task_list = client
        .task_list(&task_list_handler)
        .await
        .expect("task list succeeds");
    assert!(task_list.tasks.is_empty());
    {
        let calls = task_list_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "task/list");
    }

    let background_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(BackgroundAllTasksResult {
            task_ids: vec!["task-1".to_string()],
        })
        .expect("background-all result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let backgrounded = client
        .background_all_tasks(&background_handler)
        .await
        .expect("background-all succeeds");
    assert_eq!(backgrounded.task_ids, vec!["task-1".to_string()]);
    let calls = background_handler.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "control/backgroundAllTasks");
}

#[tokio::test]
async fn local_server_client_maps_dispatch_errors_to_server_errors() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = ServerClient::connect_local(&adapter);
    let handler = RecordingLocalRequestHandler {
        error: Some(LocalClientDispatchError::invalid_params(
            "bad local request",
        )),
        ..RecordingLocalRequestHandler::default()
    };

    let Err(ClientError::Server { message, .. }) = client.keep_alive(&handler).await else {
        panic!("expected server error");
    };

    assert_eq!(message, "bad local request");
}

#[tokio::test]
async fn remote_json_rpc_client_correlates_success_response() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let request_task = tokio::spawn(async move {
        client
            .send_client_request(ClientRequest::KeepAlive)
            .await
            .expect("request succeeds")
    });
    let frame = outbound_rx.recv().await.expect("outbound request");
    let JsonRpcFrame::Request(request) = frame else {
        panic!("expected request frame");
    };
    assert_eq!(request.method, "control/keepAlive");

    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "ok": true }),
        )))
        .await
        .expect("handle success");

    assert_eq!(
        request_task.await.expect("request task"),
        serde_json::json!({ "ok": true })
    );
}

#[tokio::test]
async fn remote_json_rpc_client_typed_methods_encode_and_decode_results() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
    let start_client = client.clone();

    let start_task = tokio::spawn(async move {
        start_client
            .session_start(SessionStartParams::default())
            .await
            .expect("session start succeeds")
    });
    let JsonRpcFrame::Request(start_request) =
        outbound_rx.recv().await.expect("outbound session/start")
    else {
        panic!("expected request frame");
    };
    assert_eq!(start_request.method, "session/start");
    let session_id = test_session_id("sess-typed-client");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            start_request.id,
            serde_json::json!({ "session_id": session_id }),
        )))
        .await
        .expect("handle session/start response");
    assert_eq!(
        start_task.await.expect("start task").session_id,
        test_session_id("sess-typed-client")
    );

    let interrupt_client = client.clone();
    let interrupt_task = tokio::spawn(async move { interrupt_client.turn_interrupt().await });
    let JsonRpcFrame::Request(interrupt_request) =
        outbound_rx.recv().await.expect("outbound turn/interrupt")
    else {
        panic!("expected request frame");
    };
    assert_eq!(interrupt_request.method, "turn/interrupt");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            interrupt_request.id,
            serde_json::Value::Null,
        )))
        .await
        .expect("handle turn/interrupt response");
    interrupt_task
        .await
        .expect("interrupt task")
        .expect("interrupt succeeds");

    let input_client = client.clone();
    let input_task = tokio::spawn(async move {
        input_client
            .user_input_resolve(UserInputResolveParams {
                request_id: "input-1".to_string(),
                answer: "yes".to_string(),
            })
            .await
    });
    let JsonRpcFrame::Request(input_request) =
        outbound_rx.recv().await.expect("outbound input/resolve")
    else {
        panic!("expected request frame");
    };
    assert_eq!(input_request.method, "input/resolveUserInput");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            input_request.id,
            serde_json::Value::Null,
        )))
        .await
        .expect("handle input/resolve response");
    input_task
        .await
        .expect("input task")
        .expect("input resolve succeeds");

    let status_client = client.clone();
    let status_task = tokio::spawn(async move { status_client.mcp_status().await });
    let JsonRpcFrame::Request(status_request) =
        outbound_rx.recv().await.expect("outbound mcp/status")
    else {
        panic!("expected request frame");
    };
    assert_eq!(status_request.method, "mcp/status");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            status_request.id,
            serde_json::json!({ "mcpServers": [] }),
        )))
        .await
        .expect("handle mcp/status response");
    assert!(
        status_task
            .await
            .expect("status task")
            .expect("status succeeds")
            .mcp_servers
            .is_empty()
    );

    let session_status_client = client.clone();
    let session_status_task =
        tokio::spawn(async move { session_status_client.session_status().await });
    let JsonRpcFrame::Request(session_status_request) =
        outbound_rx.recv().await.expect("outbound session/status")
    else {
        panic!("expected request frame");
    };
    assert_eq!(session_status_request.method, "session/status");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            session_status_request.id,
            serde_json::json!({ "text": "ready" }),
        )))
        .await
        .expect("handle session/status response");
    assert_eq!(
        session_status_task
            .await
            .expect("session status task")
            .expect("session status succeeds")
            .text,
        "ready"
    );

    let task_detail_client = client.clone();
    let task_detail_task = tokio::spawn(async move {
        task_detail_client
            .task_detail(TaskDetailParams {
                task_id: "task-1".to_string(),
            })
            .await
    });
    let JsonRpcFrame::Request(task_detail_request) =
        outbound_rx.recv().await.expect("outbound task/detail")
    else {
        panic!("expected request frame");
    };
    assert_eq!(task_detail_request.method, "task/detail");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            task_detail_request.id,
            serde_json::json!({
                "task_id": "task-1",
                "stdout": "done\n",
                "stderr": "",
                "exit_code": 0,
                "interrupted": false
            }),
        )))
        .await
        .expect("handle task/detail response");
    let detail = task_detail_task
        .await
        .expect("task detail task")
        .expect("task detail succeeds");
    assert_eq!(detail.task_id, "task-1");
    assert_eq!(detail.stdout, "done\n");

    let apply_client = client.clone();
    let apply_task = tokio::spawn(async move {
        apply_client
            .config_apply_flags(ConfigApplyFlagsParams {
                settings: HashMap::new(),
            })
            .await
    });
    let JsonRpcFrame::Request(apply_request) = outbound_rx
        .recv()
        .await
        .expect("outbound config/applyFlags")
    else {
        panic!("expected request frame");
    };
    assert_eq!(apply_request.method, "config/applyFlags");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            apply_request.id,
            serde_json::Value::Null,
        )))
        .await
        .expect("handle config/applyFlags response");
    apply_task
        .await
        .expect("apply task")
        .expect("apply succeeds");
}

#[tokio::test]
async fn remote_json_rpc_client_routes_server_error_to_pending_request() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let request_task = tokio::spawn(async move {
        client
            .request(
                "session/read",
                Some(serde_json::json!({ "session_id": "sess-1" })),
            )
            .await
    });
    let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("outbound request") else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            request.id,
            JsonRpcErrorObject::new(-32602, "bad params", None),
        )))
        .await
        .expect("handle error");

    let Err(ClientError::Server { code, message, .. }) = request_task.await.expect("request task")
    else {
        panic!("expected server error");
    };
    assert_eq!(code, -32602);
    assert_eq!(message, "bad params");
}

#[tokio::test]
async fn remote_json_rpc_client_delivers_notifications_and_disconnect_terminal_event() {
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "custom/notice",
            Some(serde_json::json!({ "surface_id": "surface-1" })),
        )))
        .await
        .expect("handle notification");
    assert!(matches!(
        events.recv().await.expect("notification event"),
        RemoteJsonRpcEvent::Notification(notification)
            if notification.method == "custom/notice"
    ));

    incoming.disconnect().await;
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
}

#[tokio::test]
async fn remote_json_rpc_client_decodes_surface_delivery_notifications() {
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);
    let session_id = test_session_id("sess-remote");
    let notification = ServerNotification::SessionStateChanged {
        state: SessionState::Running,
    };

    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/event",
            Some(serde_json::json!({
                "surface_id": "surface-remote",
                "envelope": {
                    "session_id": session_id,
                    "agent_id": null,
                    "turn_id": null,
                    "session_seq": 5,
                    "event": {
                        "layer": "protocol",
                        "payload": notification,
                    },
                },
            })),
        )))
        .await
        .expect("handle surface delivery notification");

    let RemoteJsonRpcEvent::SurfaceDelivery(delivery) =
        events.recv().await.expect("surface delivery")
    else {
        panic!("expected surface delivery");
    };
    assert_eq!(delivery.surface_id, SurfaceId::from("surface-remote"));
    assert_eq!(delivery.envelope.session_seq, Some(5));
    assert!(matches!(
        delivery.envelope.event,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running
        })
    ));
}

#[tokio::test]
async fn remote_json_rpc_client_decodes_lifecycle_notifications() {
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/lifecycle",
            Some(serde_json::json!({
                "surface_id": "surface-remote",
                "effect": {
                    "type": "session_ended",
                    "session_id": "sess-ended",
                },
            })),
        )))
        .await
        .expect("handle lifecycle notification");

    let RemoteJsonRpcEvent::SurfaceLifecycle(delivery) =
        events.recv().await.expect("lifecycle delivery")
    else {
        panic!("expected lifecycle delivery");
    };
    assert_eq!(delivery.surface_id, SurfaceId::from("surface-remote"));
    assert_eq!(
        delivery.effect.kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: test_session_id("sess-ended")
        }
    );
}

#[tokio::test]
async fn remote_json_rpc_client_surfaces_server_requests_and_sends_replies() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

    incoming
        .handle_frame(JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("server-req-1".to_string()),
            "input/requestUserInput",
            Some(serde_json::json!({ "prompt": "continue?" })),
        )))
        .await
        .expect("handle server request");

    let RemoteJsonRpcEvent::ServerRequest(request) =
        events.recv().await.expect("server request event")
    else {
        panic!("expected server request event");
    };
    assert_eq!(request.method, "input/requestUserInput");

    client
        .reply_server_request_success(request.id.clone(), serde_json::json!({ "ok": true }))
        .await
        .expect("send success reply");
    let JsonRpcFrame::Success(success) = outbound_rx.recv().await.expect("success reply") else {
        panic!("expected success reply");
    };
    assert_eq!(success.id, request.id);
    assert_eq!(success.result, serde_json::json!({ "ok": true }));

    client
        .reply_server_request_error(
            JsonRpcId::String("server-req-2".to_string()),
            -32603,
            "failed",
            None,
        )
        .await
        .expect("send error reply");
    let JsonRpcFrame::Error(error) = outbound_rx.recv().await.expect("error reply") else {
        panic!("expected error reply");
    };
    assert_eq!(error.id, JsonRpcId::String("server-req-2".to_string()));
    assert_eq!(error.error.code, -32603);
    assert_eq!(error.error.message, "failed");
}

#[tokio::test]
async fn remote_json_rpc_disconnect_resolves_pending_and_invalidates_client() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);
    let request_client = client.clone();

    let request_task = tokio::spawn(async move { request_client.request("slow", None).await });
    let JsonRpcFrame::Request(_request) = outbound_rx.recv().await.expect("outbound request")
    else {
        panic!("expected request frame");
    };

    incoming.disconnect().await;

    assert!(matches!(
        request_task.await.expect("request task"),
        Err(ClientError::Disconnected)
    ));
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
    assert!(matches!(
        client.request("after/disconnect", None).await,
        Err(ClientError::ClientInvalid)
    ));
}

#[tokio::test]
async fn remote_ndjson_connection_drives_request_response_and_disconnect() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = split(client_stream);
    let (server_read, server_write) = split(server_stream);
    let client_transport = NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
    let mut server_transport =
        NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
    let (client, connection, mut events) = RemoteJsonRpcClient::connect_ndjson(client_transport);
    let connection_task = tokio::spawn(connection.run());
    let request_client = client.clone();

    let request_task =
        tokio::spawn(async move { request_client.request("control/keepAlive", None).await });
    let Some(JsonRpcFrame::Request(request)) = server_transport
        .recv_frame()
        .await
        .expect("server reads request")
    else {
        panic!("expected request frame");
    };
    server_transport
        .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "ok": true }),
        )))
        .await
        .expect("server writes response");

    assert_eq!(
        request_task
            .await
            .expect("request task")
            .expect("request success"),
        serde_json::json!({ "ok": true })
    );
    drop(server_transport);

    connection_task
        .await
        .expect("connection task")
        .expect("connection exits cleanly");
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
    assert!(matches!(
        client.request("after/disconnect", None).await,
        Err(ClientError::ClientInvalid)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn remote_json_rpc_client_connects_over_unix_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
        .expect("bind unix listener");
    let server_task =
        tokio::spawn(async move { listener.accept().await.expect("accept unix stream") });

    let (client, connection, mut events) = RemoteJsonRpcClient::connect_unix_with_options(
        &socket_path,
        RemoteConnectOptions {
            outbound_channel_capacity: 8,
            event_channel_capacity: 8,
        },
    )
    .await
    .expect("connect unix socket");
    let mut server_transport = server_task.await.expect("server task");
    let connection_task = tokio::spawn(connection.run());
    let request_client = client.clone();

    let request_task =
        tokio::spawn(async move { request_client.request("control/keepAlive", None).await });
    let Some(JsonRpcFrame::Request(request)) = server_transport
        .recv_frame()
        .await
        .expect("server reads request")
    else {
        panic!("expected request frame");
    };
    server_transport
        .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "ok": true }),
        )))
        .await
        .expect("server writes response");

    assert_eq!(
        request_task
            .await
            .expect("request task")
            .expect("request success"),
        serde_json::json!({ "ok": true })
    );
    drop(server_transport);

    connection_task
        .await
        .expect("connection task")
        .expect("connection exits cleanly");
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
    assert!(matches!(
        client.request("after/disconnect", None).await,
        Err(ClientError::ClientInvalid)
    ));
}

#[test]
fn remote_event_demux_buffers_mixed_events_by_surface() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let mut demux = RemoteEventDemux::new(events_rx);
    let first = SurfaceId::from("surface-first");
    let second = SurfaceId::from("surface-second");
    let first_session = test_session_id("sess-first");
    let second_session = test_session_id("sess-second");

    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: second.clone(),
                envelope: durable_envelope(second_session.clone(), 2),
            },
        )))
        .expect("send second event");
    events_tx
        .try_send(RemoteJsonRpcEvent::ServerRequest(JsonRpcRequest::new(
            JsonRpcId::String("server-req".to_string()),
            "input/requestUserInput",
            Some(serde_json::json!({ "prompt": "continue?" })),
        )))
        .expect("send server request");
    events_tx
        .try_send(RemoteJsonRpcEvent::Notification(JsonRpcNotification::new(
            "custom/notice",
            Some(serde_json::json!({ "ok": true })),
        )))
        .expect("send notification");
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceLifecycle(
            SurfaceLifecycleDelivery {
                surface_id: second.clone(),
                effect: SurfaceLifecycleEffect {
                    surface_id: second.clone(),
                    kind: SurfaceLifecycleEffectKind::SessionEnded {
                        session_id: second_session.clone(),
                    },
                },
            },
        ))
        .expect("send lifecycle");
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: first.clone(),
                envelope: durable_envelope(first_session.clone(), 1),
            },
        )))
        .expect("send first event");
    events_tx
        .try_send(RemoteJsonRpcEvent::Disconnected)
        .expect("send disconnect");

    let first_event = demux
        .try_next_surface_event(&first)
        .expect("first surface event");
    assert_eq!(first_event.session_id, first_session);

    let second_event = demux
        .try_next_surface_event(&second)
        .expect("second surface event");
    assert_eq!(second_event.session_id, second_session);

    let server_request = demux
        .try_next_server_request()
        .expect("server request was buffered");
    assert_eq!(server_request.method, "input/requestUserInput");

    let notification = demux
        .try_next_notification()
        .expect("notification was buffered");
    assert_eq!(notification.method, "custom/notice");

    let lifecycle = demux
        .try_next_lifecycle(&second)
        .expect("lifecycle was buffered");
    assert_eq!(
        lifecycle.effect.kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: second_session
        }
    );

    assert!(demux.try_next_surface_event(&first).is_none());
    assert!(demux.is_disconnected());
}

#[test]
fn remote_surface_stream_reads_events_and_lifecycle_for_one_surface() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let mut demux = RemoteEventDemux::new(events_rx);
    let surface = SurfaceId::from("surface-stream");
    let session = test_session_id("sess-stream");

    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: surface.clone(),
                envelope: durable_envelope(session.clone(), 1),
            },
        )))
        .expect("send surface event");
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceLifecycle(
            SurfaceLifecycleDelivery {
                surface_id: surface.clone(),
                effect: SurfaceLifecycleEffect {
                    surface_id: surface.clone(),
                    kind: SurfaceLifecycleEffectKind::SessionEnded {
                        session_id: session.clone(),
                    },
                },
            },
        ))
        .expect("send lifecycle");

    let mut stream = demux.surface_stream(surface.clone());
    assert_eq!(stream.surface_id(), &surface);
    assert_eq!(
        stream.try_next_event().expect("surface event").session_id,
        session
    );
    assert_eq!(
        stream.try_next_lifecycle().expect("lifecycle").effect.kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: test_session_id("sess-stream")
        }
    );
}

#[tokio::test]
async fn remote_event_demux_async_methods_wait_and_buffer_mixed_events() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let mut demux = RemoteEventDemux::new(events_rx);
    let first = SurfaceId::from("surface-first");
    let second = SurfaceId::from("surface-second");
    let first_session = test_session_id("sess-first");
    let second_session = test_session_id("sess-second");

    events_tx
        .send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: second.clone(),
                envelope: durable_envelope(second_session.clone(), 2),
            },
        )))
        .await
        .expect("send second event");
    events_tx
        .send(RemoteJsonRpcEvent::ServerRequest(JsonRpcRequest::new(
            JsonRpcId::String("server-req".to_string()),
            "input/requestUserInput",
            Some(serde_json::json!({ "prompt": "continue?" })),
        )))
        .await
        .expect("send server request");
    events_tx
        .send(RemoteJsonRpcEvent::Notification(JsonRpcNotification::new(
            "custom/notice",
            Some(serde_json::json!({ "ok": true })),
        )))
        .await
        .expect("send notification");
    events_tx
        .send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: first.clone(),
                envelope: durable_envelope(first_session.clone(), 1),
            },
        )))
        .await
        .expect("send first event");
    events_tx
        .send(RemoteJsonRpcEvent::Disconnected)
        .await
        .expect("send disconnect");

    let first_event = demux.next_surface_event(&first).await.expect("first event");
    assert_eq!(first_event.session_id, first_session);
    let second_event = demux
        .next_surface_event(&second)
        .await
        .expect("second event");
    assert_eq!(second_event.session_id, second_session);
    let server_request = demux.next_server_request().await.expect("server request");
    assert_eq!(server_request.method, "input/requestUserInput");
    let notification = demux.next_notification().await.expect("notification");
    assert_eq!(notification.method, "custom/notice");
    assert!(demux.next_surface_event(&first).await.is_none());
    assert!(demux.is_disconnected());
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

#[tokio::test]
async fn local_server_client_next_event_buffers_other_surfaces() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let first_session = test_session_id("sess-1");
    let second_session = test_session_id("sess-2");
    for session_id in [&first_session, &second_session] {
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(session_id, TestHandle("handle"))
            .expect("session live");
    }
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = ServerClient::connect_local(&adapter);
    let first = client
        .subscribe_session(
            first_session.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe first");
    let second = client
        .subscribe_session(
            second_session.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe second");

    server.route_envelope(durable_envelope(second_session.clone(), 1));
    server.route_envelope(durable_envelope(first_session.clone(), 1));

    let first_event = client
        .next_passive_event(&first)
        .await
        .expect("first event");
    assert_eq!(first_event.session_id, first_session);
    let buffered_second = client
        .try_next_passive_event(&second)
        .expect("buffered second event");
    assert_eq!(buffered_second.session_id, second_session);
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
            first_session_id,
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
                session_id: passive_session_id,
            },
        },
        SurfaceLifecycleEffect {
            surface_id: interactive.surface_id().clone(),
            kind: SurfaceLifecycleEffectKind::SessionStarted {
                session_id: interactive_session_id,
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
