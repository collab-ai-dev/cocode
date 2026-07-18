use coco_app_server_transport::JsonRpcNotification;
use coco_types::{
    CoreEvent, ServerNotification, SessionEnvelope, SessionState, SurfaceDelivery,
    SurfaceLifecycleEffectKind, TurnId,
};
use tokio::io::{BufReader, split};

use super::*;

fn test_session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid test session id")
}

fn test_session_target(value: &str) -> SessionTarget {
    SessionTarget {
        session_id: test_session_id(value),
    }
}

fn test_interactive_target() -> InteractiveTarget {
    InteractiveTarget {
        session_id: test_session_id("sess-typed-client"),
        surface_id: SurfaceId::from("surface-typed-client"),
    }
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
            serde_json::json!({ "session_id": session_id, "surface_id": "surface-typed-client" }),
        )))
        .await
        .expect("handle session/start response");
    assert_eq!(
        start_task.await.expect("start task").session_id,
        test_session_id("sess-typed-client")
    );

    let interrupt_client = client.clone();
    let interrupt_task = tokio::spawn(async move {
        interrupt_client
            .turn_interrupt(test_interactive_target())
            .await
    });
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
                target: test_interactive_target(),
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
    let status_task = tokio::spawn(async move {
        status_client
            .mcp_status(test_session_target("sess-typed-client"))
            .await
    });
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
    let session_status_task = tokio::spawn(async move {
        session_status_client
            .session_status(test_session_target("sess-typed-client"))
            .await
    });
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

    let turns_client = client.clone();
    let turns_session_id = session_id.clone();
    let turns_task = tokio::spawn(async move {
        turns_client
            .session_turns_list(SessionTurnsListParams {
                target: SessionTarget {
                    session_id: turns_session_id,
                },
                cursor: Some("1".to_string()),
                limit: Some(2),
            })
            .await
    });
    let JsonRpcFrame::Request(turns_request) = outbound_rx
        .recv()
        .await
        .expect("outbound session/turns/list")
    else {
        panic!("expected request frame");
    };
    assert_eq!(turns_request.method, "session/turns/list");
    assert_eq!(
        turns_request
            .params
            .as_ref()
            .and_then(|params| params.get("cursor"))
            .and_then(serde_json::Value::as_str),
        Some("1")
    );
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            turns_request.id,
            serde_json::json!({
                "session": {
                    "session_id": session_id,
                    "model": "gpt-test",
                    "cwd": "/tmp",
                    "created_at": "2026-07-08T00:00:00Z",
                    "message_count": 2,
                    "total_tokens": 0
                },
                "turns": [{
                    "index": 1,
                    "start_cursor": "2",
                    "message_count": 2
                }],
                "has_more": false
            }),
        )))
        .await
        .expect("handle session/turns/list response");
    assert_eq!(
        turns_task
            .await
            .expect("turns task")
            .expect("turns list succeeds")
            .turns[0]
            .start_cursor,
        "2"
    );

    let task_detail_client = client.clone();
    let task_detail_task = tokio::spawn(async move {
        task_detail_client
            .task_detail(TaskDetailParams {
                target: test_session_target("sess-typed-client"),
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
                target: test_interactive_target(),
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

    let Err(ClientError::InvalidParams { message, .. }) = request_task.await.expect("request task")
    else {
        panic!("expected invalid params error");
    };
    assert_eq!(message, "bad params");
}

#[tokio::test]
async fn remote_json_rpc_client_maps_standard_server_error_codes() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let method_task = {
        let client = client.clone();
        tokio::spawn(async move { client.request("missing/method", None).await })
    };
    let JsonRpcFrame::Request(method_request) = outbound_rx.recv().await.expect("method request")
    else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            method_request.id,
            JsonRpcErrorObject::new(
                coco_types::error_codes::METHOD_NOT_FOUND,
                "missing method",
                Some(serde_json::json!({ "method": "missing/method" })),
            ),
        )))
        .await
        .expect("handle method error");
    let Err(ClientError::MethodNotFound { message, data }) =
        method_task.await.expect("method task")
    else {
        panic!("expected method not found error");
    };
    assert_eq!(message, "missing method");
    assert_eq!(
        data.and_then(|data| data.get("method").cloned()),
        Some(serde_json::json!("missing/method"))
    );

    let internal_task = tokio::spawn(async move { client.request("boom", None).await });
    let JsonRpcFrame::Request(internal_request) =
        outbound_rx.recv().await.expect("internal request")
    else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            internal_request.id,
            JsonRpcErrorObject::new(
                coco_types::error_codes::INTERNAL_ERROR,
                "server exploded",
                None,
            ),
        )))
        .await
        .expect("handle internal error");
    assert!(matches!(
        internal_task.await.expect("internal task"),
        Err(ClientError::InternalServerError { message, data: None })
            if message == "server exploded"
    ));
}

#[tokio::test]
async fn remote_json_rpc_client_maps_surface_limit_error_kind() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let request_task = tokio::spawn(async move { client.request("session/attach", None).await });
    let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("outbound request") else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            request.id,
            JsonRpcErrorObject::new(
                coco_types::error_codes::INVALID_REQUEST,
                "surface limit reached",
                Some(serde_json::json!({
                    "kind": "surface_limit",
                    "max": 8,
                })),
            ),
        )))
        .await
        .expect("handle error");

    let Err(ClientError::SurfaceLimit { message, data }) =
        request_task.await.expect("request task")
    else {
        panic!("expected surface limit error");
    };
    assert_eq!(message, "surface limit reached");
    assert_eq!(
        data.and_then(|data| data.get("max").cloned()),
        Some(serde_json::json!(8))
    );
}

#[tokio::test]
async fn remote_json_rpc_client_preserves_unknown_domain_error_kind() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let request_task = tokio::spawn(async move { client.request("session/attach", None).await });
    let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("outbound request") else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            request.id,
            JsonRpcErrorObject::new(
                coco_types::error_codes::INVALID_REQUEST,
                "unknown domain failure",
                Some(serde_json::json!({
                    "kind": "future_domain_error",
                    "retry_after_ms": 25,
                })),
            ),
        )))
        .await
        .expect("handle error");

    let Err(ClientError::Domain {
        code,
        kind,
        message,
        data,
    }) = request_task.await.expect("request task")
    else {
        panic!("expected domain error");
    };
    assert_eq!(code, coco_types::error_codes::INVALID_REQUEST);
    assert_eq!(kind, "future_domain_error");
    assert_eq!(message, "unknown domain failure");
    assert_eq!(
        data.and_then(|data| data.get("retry_after_ms").cloned()),
        Some(serde_json::json!(25))
    );
}

#[tokio::test]
async fn remote_json_rpc_client_preserves_unknown_server_error_code() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let request_task = tokio::spawn(async move { client.request("custom/error", None).await });
    let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("outbound request") else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            request.id,
            JsonRpcErrorObject::new(-32042, "custom failure", None),
        )))
        .await
        .expect("handle error");

    let Err(ClientError::Server { code, message, .. }) = request_task.await.expect("request task")
    else {
        panic!("expected generic server error");
    };
    assert_eq!(code, -32042);
    assert_eq!(message, "custom failure");
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
        delivery.kind,
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
async fn remote_incoming_tolerates_unknown_response_id() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    // A success/error for an id that was never pending (a late reply after a
    // per-request timeout, or a duplicate) is tolerate-with-warn: handle_frame
    // returns Ok and the client stays valid.
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::Number(999),
            serde_json::json!({ "late": true }),
        )))
        .await
        .expect("unknown success tolerated");
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            JsonRpcId::Number(1000),
            JsonRpcErrorObject::new(-32000, "late", None),
        )))
        .await
        .expect("unknown error tolerated");

    // A real request over the still-valid client still correlates.
    let request_client = client.clone();
    let request_task =
        tokio::spawn(async move { request_client.request("keep/alive", None).await });
    let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("request") else {
        panic!("expected request frame");
    };
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id,
            serde_json::json!({ "ok": true }),
        )))
        .await
        .expect("handle response");
    assert_eq!(
        request_task.await.expect("request task").expect("ok"),
        serde_json::json!({ "ok": true })
    );
}

#[tokio::test]
async fn remote_incoming_drop_resolves_pending_and_invalidates() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);
    let request_client = client.clone();
    let request_task = tokio::spawn(async move { request_client.request("slow", None).await });
    let JsonRpcFrame::Request(_request) = outbound_rx.recv().await.expect("request") else {
        panic!("expected request frame");
    };

    // Dropping the incoming half WITHOUT a graceful disconnect() (the standard
    // aborted-owner-task shutdown move) must still resolve the in-flight RPC with
    // Disconnected, emit the terminal event, and invalidate the client.
    drop(incoming);

    assert!(matches!(
        request_task.await.expect("request task"),
        Err(ClientError::Disconnected)
    ));
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
    assert!(matches!(
        client.request("after/drop", None).await,
        Err(ClientError::ClientInvalid)
    ));
}

#[tokio::test]
async fn remote_incoming_tolerates_undecodable_lifecycle_notification() {
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

    // An unknown lifecycle effect kind (a newer server) is dropped with a
    // warning, not fatal: handle_frame returns Ok and emits no event.
    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/lifecycle",
            Some(serde_json::json!({
                "surface_id": "surface-x",
                "effect": { "type": "session_teleported", "session_id": "sess-x" }
            })),
        )))
        .await
        .expect("undecodable lifecycle tolerated");

    // A subsequent well-formed notification is still delivered on the same
    // still-live connection.
    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "custom/notice",
            Some(serde_json::json!({ "ok": true })),
        )))
        .await
        .expect("custom notification delivered");
    assert!(matches!(
        events.recv().await.expect("event"),
        RemoteJsonRpcEvent::Notification(_)
    ));
}
#[test]
fn remote_demux_purge_surface_drops_buffered_events_and_lifecycle() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let mut demux = RemoteEventDemux::new(events_rx);
    let target = SurfaceId::from("surface-target");
    let other = SurfaceId::from("surface-other");
    let target_session = test_session_id("sess-target");
    let other_session = test_session_id("sess-other");

    // Buffer a delivery + lifecycle for `target` while reading `other`.
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: target.clone(),
                envelope: durable_envelope(target_session.clone(), 1),
            },
        )))
        .expect("send target event");
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceLifecycle(
            SurfaceLifecycleEffect {
                surface_id: target.clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: target_session,
                },
            },
        ))
        .expect("send target lifecycle");
    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: other.clone(),
                envelope: durable_envelope(other_session.clone(), 1),
            },
        )))
        .expect("send other event");

    // Reading `other` buffers the two `target` deliveries.
    let other_event = demux.try_next_surface_event(&other).expect("other event");
    assert_eq!(other_event.session_id, other_session);

    // Purge drops the buffered target queues; nothing is left to read.
    demux.purge_surface(&target);
    assert!(demux.try_next_surface_event(&target).is_none());
    assert!(demux.try_next_lifecycle(&target).is_none());
}

#[tokio::test]
async fn remote_ndjson_write_timeout_triggers_slow_consumer_disconnect() {
    // A 16-byte pipe whose server end stays open but never reads: the first frame
    // write stalls past `write_timeout`, so the owner fails with `SlowConsumer`
    // and the guaranteed disconnect resolves the in-flight request.
    let (client_stream, _server_stream) = tokio::io::duplex(16);
    let (client_read, client_write) = split(client_stream);
    let client_transport = NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
    let (client, connection, _events) = RemoteJsonRpcClient::connect_ndjson_with_options(
        client_transport,
        RemoteConnectOptions {
            outbound_channel_capacity: 8,
            event_channel_capacity: 8,
            request_timeout: None,
            write_timeout: Some(Duration::from_millis(50)),
        },
    );
    let connection_task = tokio::spawn(connection.run());
    let request_client = client.clone();
    let request_task =
        tokio::spawn(async move { request_client.request("control/keepAlive", None).await });

    assert!(matches!(
        connection_task.await.expect("connection task"),
        Err(RemoteTransportError::SlowConsumer)
    ));
    assert!(matches!(
        request_task.await.expect("request task"),
        Err(ClientError::Disconnected)
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
            request_timeout: None,
            write_timeout: None,
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

#[cfg(unix)]
#[tokio::test]
async fn connect_unix_to_missing_socket_returns_connect_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("missing.sock");

    let error = RemoteJsonRpcClient::connect_unix(&socket_path)
        .await
        .err()
        .expect("connect to missing socket fails");

    assert!(error.to_string().starts_with("connection failed: "));
    let ClientError::Connect(message) = error else {
        panic!("expected ClientError::Connect");
    };
    assert!(!message.is_empty(), "underlying error text is preserved");
}

#[tokio::test]
async fn remote_request_timeout_returns_timeout_and_clears_pending() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = split(client_stream);
    let (server_read, server_write) = split(server_stream);
    let client_transport = NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
    let mut server_transport =
        NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
    let (client, connection, _events) = RemoteJsonRpcClient::connect_ndjson_with_options(
        client_transport,
        RemoteConnectOptions {
            outbound_channel_capacity: 8,
            event_channel_capacity: 8,
            request_timeout: Some(Duration::from_millis(25)),
            write_timeout: None,
        },
    );
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
    let late_id = request.id.clone();

    // The server never responds: the request must resolve with Timeout.
    assert!(matches!(
        request_task.await.expect("request task"),
        Err(ClientError::Timeout)
    ));
    // The timed-out id must be removed from the correlation map.
    assert!(lock_pending(&client.pending).is_empty());

    // A late response for the timed-out id is tolerate-with-warn: it hits
    // the unknown-response-id contract and is dropped, NOT fatal. The connection
    // keeps running and a subsequent live request still correlates.
    server_transport
        .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
            late_id,
            serde_json::json!({ "late": true }),
        )))
        .await
        .expect("server writes late response");

    // A fresh request over the still-live connection correlates normally.
    let next_client = client.clone();
    let next_task =
        tokio::spawn(async move { next_client.request("control/keepAlive", None).await });
    let Some(JsonRpcFrame::Request(next_request)) = server_transport
        .recv_frame()
        .await
        .expect("server reads next request")
    else {
        panic!("expected second request frame");
    };
    server_transport
        .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
            next_request.id,
            serde_json::json!({ "ok": true }),
        )))
        .await
        .expect("server writes response");
    assert_eq!(
        next_task.await.expect("next request task").expect("ok"),
        serde_json::json!({ "ok": true })
    );

    // Dropping the client closes the outbound channel; the owner exits cleanly.
    drop(client);
    assert!(matches!(
        connection_task.await.expect("connection task"),
        Ok(())
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
            SurfaceLifecycleEffect {
                surface_id: second.clone(),
                kind: SurfaceLifecycleEffectKind::SessionEnded {
                    session_id: second_session.clone(),
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
        lifecycle.kind,
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
            SurfaceLifecycleEffect {
                surface_id: surface.clone(),
                kind: SurfaceLifecycleEffectKind::SessionEnded {
                    session_id: session.clone(),
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
        stream.try_next_lifecycle().expect("lifecycle").kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: test_session_id("sess-stream")
        }
    );
}

#[test]
fn remote_session_handles_read_surface_events_through_demux() {
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (client, _incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
    let (events_tx, events_rx) = mpsc::channel(8);
    let mut demux = RemoteEventDemux::new(events_rx);
    let session = test_session_id("sess-remote-handle");
    let surface = SurfaceId::from("surface-remote-handle");
    let remote_session = client.session_handle(session.clone(), surface.clone());

    events_tx
        .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: surface.clone(),
                envelope: durable_envelope(session.clone(), 1),
            },
        )))
        .expect("send remote session event");

    assert_eq!(remote_session.session_id(), &session);
    assert_eq!(remote_session.surface_id(), &surface);
    assert_eq!(
        remote_session
            .try_next_event(&mut demux)
            .expect("remote session event")
            .session_id,
        session
    );
}

#[tokio::test]
async fn remote_session_start_handle_uses_result_surface_id() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, events) = RemoteJsonRpcClient::new(outbound_tx);
    let mut demux = RemoteEventDemux::new(events);
    let start_client = client.clone();
    let start_task = tokio::spawn(async move {
        start_client
            .session_start_handle(&mut demux, SessionStartParams::default())
            .await
    });

    let JsonRpcFrame::Request(start_request) = outbound_rx.recv().await.expect("start request")
    else {
        panic!("expected start request");
    };
    assert_eq!(start_request.method, "session/start");
    let session_id = test_session_id("sess-remote-start-handle");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            start_request.id,
            serde_json::json!({
                "session_id": session_id,
                "surface_id": "surface-remote-start-handle",
            }),
        )))
        .await
        .expect("handle start response");

    let remote_session = start_task.await.expect("start task").expect("start handle");
    assert_eq!(remote_session.session_id(), &session_id);
    assert_eq!(
        remote_session.surface_id(),
        &SurfaceId::from("surface-remote-start-handle")
    );
}

#[tokio::test]
async fn remote_session_resume_handle_uses_result_surface_id() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, events) = RemoteJsonRpcClient::new(outbound_tx);
    let mut demux = RemoteEventDemux::new(events);
    let resume_client = client.clone();
    let resume_task = tokio::spawn(async move {
        resume_client
            .session_resume_handle(
                &mut demux,
                SessionResumeParams {
                    target: test_session_target("sess-remote-resume-handle"),
                    plan_mode_instructions: None,
                },
            )
            .await
    });

    let JsonRpcFrame::Request(resume_request) = outbound_rx.recv().await.expect("resume request")
    else {
        panic!("expected resume request");
    };
    assert_eq!(resume_request.method, "session/resume");
    let session_id = test_session_id("sess-remote-resume-handle");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            resume_request.id,
            serde_json::json!({
                "session": {
                    "session_id": session_id,
                    "model": "gpt-test",
                    "cwd": "/tmp",
                    "created_at": "2026-07-08T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0
                },
                "surface_id": "surface-remote-resume-handle"
            }),
        )))
        .await
        .expect("handle resume response");

    let remote_session = resume_task
        .await
        .expect("resume task")
        .expect("resume handle");
    assert_eq!(remote_session.session_id(), &session_id);
    assert_eq!(
        remote_session.surface_id(),
        &SurfaceId::from("surface-remote-resume-handle")
    );
}

#[tokio::test]
async fn remote_session_replace_resume_uses_lifecycle_fallback() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, events) = RemoteJsonRpcClient::new(outbound_tx);
    let mut demux = RemoteEventDemux::new(events);
    let old_session = client.session_handle(
        test_session_id("sess-remote-replace-old"),
        SurfaceId::from("surface-remote-replace-old"),
    );
    let replace_task = tokio::spawn(async move {
        old_session
            .replace_with_resume(
                &mut demux,
                SessionResumeParams {
                    target: test_session_target("sess-remote-replace-new"),
                    plan_mode_instructions: None,
                },
            )
            .await
    });

    let JsonRpcFrame::Request(resume_request) = outbound_rx.recv().await.expect("replace request")
    else {
        panic!("expected resume request");
    };
    assert_eq!(resume_request.method, "session/replace");
    let new_session_id = test_session_id("sess-remote-replace-new");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            resume_request.id,
            serde_json::json!({
                "session_id": new_session_id,
                "surface_id": "surface-remote-replace-old"
            }),
        )))
        .await
        .expect("handle resume response");
    let Ok(replaced) = replace_task.await.expect("replace task") else {
        panic!("expected replace success");
    };
    assert_eq!(replaced.session_id(), &new_session_id);
    assert_eq!(
        replaced.surface_id(),
        &SurfaceId::from("surface-remote-replace-old")
    );
}

#[tokio::test]
async fn remote_session_replace_start_returns_original_handle_on_failure() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, events) = RemoteJsonRpcClient::new(outbound_tx);
    let mut demux = RemoteEventDemux::new(events);
    let old_session_id = test_session_id("sess-remote-replace-fail-old");
    let old_surface_id = SurfaceId::from("surface-remote-replace-fail-old");
    let old_session = client.session_handle(old_session_id.clone(), old_surface_id.clone());
    let replace_task = tokio::spawn(async move {
        old_session
            .replace_with_start(&mut demux, SessionStartParams::default())
            .await
    });

    let JsonRpcFrame::Request(start_request) = outbound_rx.recv().await.expect("start request")
    else {
        panic!("expected start request");
    };
    assert_eq!(start_request.method, "session/replace");
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            start_request.id,
            JsonRpcErrorObject::new(-32602, "start failed", None),
        )))
        .await
        .expect("handle start error");

    let Err((returned, ClientError::InvalidParams { message, .. })) =
        replace_task.await.expect("replace task")
    else {
        panic!("expected replace failure");
    };
    assert_eq!(returned.session_id(), &old_session_id);
    assert_eq!(returned.surface_id(), &old_surface_id);
    assert_eq!(message, "start failed");
}

#[tokio::test]
async fn remote_session_handle_forwards_query_interrupt_and_close() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
    let session_id = test_session_id("sess-remote-query");
    let surface_id = SurfaceId::from("surface-remote-query");
    let remote_session = client.session_handle(session_id.clone(), surface_id.clone());

    // The remote handles are not `Clone`; mint fresh handles from the
    // still-cloneable client for the concurrent query/interrupt tasks.
    let query_session = client.session_handle(session_id.clone(), surface_id.clone());
    let query_task = tokio::spawn(async move {
        query_session
            .query(TurnStartParams {
                target: test_interactive_target(),
                prompt: "hello".to_string(),
                history_override: Vec::new(),
                images: Vec::new(),
                composer: Default::default(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
                goal_continuation: false,
            })
            .await
    });
    let JsonRpcFrame::Request(query_request) = outbound_rx.recv().await.expect("query request")
    else {
        panic!("expected query request");
    };
    assert_eq!(query_request.method, "turn/start");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            query_request.id,
            serde_json::json!({ "turn_id": "turn-remote" }),
        )))
        .await
        .expect("handle query response");
    assert_eq!(
        query_task
            .await
            .expect("query task")
            .expect("query succeeds")
            .turn_id,
        TurnId::from("turn-remote")
    );

    let interrupt_session = client.session_handle(session_id.clone(), surface_id.clone());
    let interrupt_task = tokio::spawn(async move { interrupt_session.interrupt().await });
    let JsonRpcFrame::Request(interrupt_request) =
        outbound_rx.recv().await.expect("interrupt request")
    else {
        panic!("expected interrupt request");
    };
    assert_eq!(interrupt_request.method, "turn/interrupt");
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            interrupt_request.id,
            serde_json::Value::Null,
        )))
        .await
        .expect("handle interrupt response");
    interrupt_task
        .await
        .expect("interrupt task")
        .expect("interrupt succeeds");

    let close_task = tokio::spawn(async move { remote_session.close().await });
    let JsonRpcFrame::Request(close_request) = outbound_rx.recv().await.expect("close request")
    else {
        panic!("expected close request");
    };
    assert_eq!(close_request.method, "session/close");
    assert_eq!(
        close_request
            .params
            .as_ref()
            .and_then(|params| params.pointer("/target/kind")),
        Some(&serde_json::json!("interactive"))
    );
    assert_eq!(
        close_request
            .params
            .as_ref()
            .and_then(|params| params.pointer("/target/target/session_id")),
        Some(&serde_json::json!(session_id))
    );
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            close_request.id,
            JsonRpcErrorObject::new(-32603, "close failed", None),
        )))
        .await
        .expect("handle close error");
    let Err((returned, ClientError::InternalServerError { message, .. })) =
        close_task.await.expect("close task")
    else {
        panic!("expected close failure to return handle");
    };
    assert_eq!(returned.session_id(), &test_session_id("sess-remote-query"));
    assert_eq!(message, "close failed");
}

#[tokio::test]
async fn remote_passive_handle_reads_session_snapshot() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
    let session_id = test_session_id("sess-remote-passive");
    let passive = client.passive_session_handle(
        session_id.clone(),
        SurfaceId::from("surface-remote-passive"),
    );

    let read_task =
        tokio::spawn(async move { passive.read(Some("12".to_string()), Some(5)).await });
    let JsonRpcFrame::Request(read_request) = outbound_rx.recv().await.expect("read request")
    else {
        panic!("expected read request");
    };
    assert_eq!(read_request.method, "session/read");
    assert_eq!(
        read_request
            .params
            .as_ref()
            .and_then(|params| params.pointer("/target/session_id")),
        Some(&serde_json::json!(session_id))
    );
    assert_eq!(
        read_request
            .params
            .as_ref()
            .and_then(|params| params.get("cursor")),
        Some(&serde_json::json!("12"))
    );
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            read_request.id,
            serde_json::json!({
                "session": {
                    "session_id": session_id,
                    "model": "gpt-test",
                    "cwd": "/tmp",
                    "created_at": "2026-07-08T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0
                },
                "messages": [],
                "has_more": false
            }),
        )))
        .await
        .expect("handle read response");

    let snapshot = read_task.await.expect("read task").expect("read succeeds");
    assert_eq!(
        snapshot.session.session_id,
        test_session_id("sess-remote-passive")
    );
    assert!(!snapshot.has_more);
}

#[tokio::test]
async fn remote_subscribe_session_returns_passive_handle_with_replay() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
    let session_id = test_session_id("sess-remote-subscribe");

    let subscribe_client = client.clone();
    let subscribe_session_id = session_id.clone();
    let subscribe_task = tokio::spawn(async move {
        subscribe_client
            .subscribe_session(subscribe_session_id, Some(6))
            .await
    });
    let JsonRpcFrame::Request(subscribe_request) =
        outbound_rx.recv().await.expect("subscribe request")
    else {
        panic!("expected subscribe request");
    };
    assert_eq!(subscribe_request.method, "session/subscribe");
    assert_eq!(
        subscribe_request
            .params
            .as_ref()
            .and_then(|params| params.pointer("/target/session_id")),
        Some(&serde_json::json!(session_id))
    );
    assert_eq!(
        subscribe_request
            .params
            .as_ref()
            .and_then(|params| params.get("after_seq")),
        Some(&serde_json::json!(6))
    );
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            subscribe_request.id,
            serde_json::to_value(coco_types::SessionSubscribeResult {
                session_id: session_id.clone(),
                surface_id: SurfaceId::from("surface-remote-subscribe"),
                replayed: vec![coco_types::SessionSubscribeEnvelope {
                    session_id: session_id.clone(),
                    agent_id: None,
                    turn_id: None,
                    session_seq: Some(7),
                    event: serde_json::json!({
                        "layer": "protocol",
                        "payload": ServerNotification::SessionStateChanged {
                            state: SessionState::Running,
                        },
                    }),
                }],
            })
            .expect("encode subscribe result"),
        )))
        .await
        .expect("handle subscribe response");

    let passive = subscribe_task
        .await
        .expect("subscribe task")
        .expect("subscribe succeeds");
    assert_eq!(passive.session_id(), &session_id);
    assert_eq!(
        passive.surface_id(),
        &SurfaceId::from("surface-remote-subscribe")
    );
    assert_eq!(passive.replayed().len(), 1);
    assert_eq!(passive.replayed()[0].session_seq, Some(7));
}

#[tokio::test]
async fn remote_subscribe_session_maps_snapshot_required_error() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

    let subscribe_task = tokio::spawn(async move {
        client
            .subscribe_session(test_session_id("sess-remote-subscribe-missing"), None)
            .await
    });
    let JsonRpcFrame::Request(subscribe_request) =
        outbound_rx.recv().await.expect("subscribe request")
    else {
        panic!("expected subscribe request");
    };
    incoming
        .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            subscribe_request.id,
            JsonRpcErrorObject::new(
                coco_types::error_codes::INVALID_REQUEST,
                "snapshot required",
                Some(serde_json::json!({ "kind": "snapshot_required" })),
            ),
        )))
        .await
        .expect("handle subscribe error");

    assert!(matches!(
        subscribe_task.await.expect("subscribe task"),
        Err(ClientError::SnapshotRequired)
    ));
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

#[tokio::test]
async fn remote_owned_surface_stream_reads_surface_and_retains_demux() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let first = SurfaceId::from("surface-owned-first");
    let second = SurfaceId::from("surface-owned-second");
    let first_session = test_session_id("sess-owned-first");
    let second_session = test_session_id("sess-owned-second");

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
            JsonRpcId::String("server-req-owned".to_string()),
            "input/requestUserInput",
            Some(serde_json::json!({ "prompt": "continue?" })),
        )))
        .await
        .expect("send server request");
    events_tx
        .send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            SurfaceDelivery {
                surface_id: first.clone(),
                envelope: durable_envelope(first_session.clone(), 1),
            },
        )))
        .await
        .expect("send first event");

    let mut stream = RemoteEventDemux::new(events_rx).into_surface_stream(first.clone());
    assert_eq!(stream.surface_id(), &first);

    let first_event = stream.next_event().await.expect("first event");
    assert_eq!(first_event.session_id, first_session);

    let demux = stream.demux_mut();
    let server_request = demux.try_next_server_request().expect("server request");
    assert_eq!(server_request.method, "input/requestUserInput");
    let second_event = demux
        .try_next_surface_event(&second)
        .expect("second event remains buffered");
    assert_eq!(second_event.session_id, second_session);
}
