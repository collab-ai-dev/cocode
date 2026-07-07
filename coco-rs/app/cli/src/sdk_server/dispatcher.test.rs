//! Dispatcher-level tests: routing, parsing, lifecycle, and the
//! CoreEvent → JsonRpcNotification translator.
//!
//! Per-handler behavior (session/*, turn/*, approval/*, control/*) is
//! tested in `handlers/tests.rs`.

use coco_types::JsonRpcMessage;
use coco_types::JsonRpcRequest;
use coco_types::RequestId;
use coco_types::ServerNotification;
use coco_types::SessionState;
use coco_types::error_codes;
use pretty_assertions::assert_eq;

use super::*;
use crate::sdk_server::InMemoryTransport;

fn req(id: i64, method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: coco_types::JSONRPC_VERSION.into(),
        request_id: RequestId::Integer(id),
        method: method.into(),
        params,
    })
}

async fn spawn_server() -> (
    tokio::task::JoinHandle<()>,
    std::sync::Arc<InMemoryTransport>,
) {
    let (server_end, client_end) = InMemoryTransport::pair(32);
    let server = SdkServer::new(server_end);
    let handle = spawn_app_server_bridge(server);
    (handle, client_end)
}

fn spawn_app_server_bridge(server: SdkServer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let app_server = std::sync::Arc::new(coco_app_server::AppServer::<()>::new(
            /*max_sessions*/ 1, /*channel_capacity*/ 32,
        ));
        let adapter = coco_app_server::JsonRpcAdapter::with_channel_capacity(app_server, 32);
        let connection = adapter.connect();
        let _ = server.run_app_server_connection(connection).await;
    })
}

#[tokio::test]
async fn keep_alive_returns_empty_ok_response() {
    let (server_task, client) = spawn_server().await;

    client
        .send(req(1, "control/keepAlive", serde_json::json!({})))
        .await
        .unwrap();

    let reply = client.recv().await.unwrap().unwrap();
    match reply {
        JsonRpcMessage::Response(r) => {
            assert_eq!(r.request_id, RequestId::Integer(1));
            assert!(r.result.is_null());
        }
        other => panic!("expected Response, got {other:?}"),
    }

    drop(client);
    server_task.await.unwrap();
}

// NOTE: `unimplemented_method_returns_method_not_found_error` was
// removed in Phase 2.C.14c. With all 29 ClientRequest variants now
// implemented, no live dispatch path returns
// `HandlerResult::NotImplemented`, so the test was asserting against
// dead code. Unknown methods (those that don't deserialize into any
// ClientRequest variant at all) are still covered by
// `unknown_method_returns_invalid_params_error`.

#[tokio::test]
async fn unknown_method_returns_invalid_params_error() {
    let (server_task, client) = spawn_server().await;

    // "nonexistent/method" is not in the ClientRequest enum, so the
    // dispatcher's serde parse will fail → INVALID_PARAMS.
    client
        .send(req(99, "nonexistent/method", serde_json::json!({})))
        .await
        .unwrap();

    let reply = client.recv().await.unwrap().unwrap();
    match reply {
        JsonRpcMessage::Error(e) => {
            assert_eq!(e.request_id, RequestId::Integer(99));
            assert_eq!(e.error.code, error_codes::INVALID_PARAMS);
        }
        other => panic!("expected Error, got {other:?}"),
    }

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn server_exits_on_eof() {
    let (server_task, client) = spawn_server().await;
    // Immediately drop the client → server sees EOF → exits cleanly.
    drop(client);
    tokio::time::timeout(std::time::Duration::from_secs(2), server_task)
        .await
        .expect("server should exit on client drop")
        .expect("server task should not panic");
}

#[tokio::test]
async fn multiple_requests_are_processed_in_order() {
    let (server_task, client) = spawn_server().await;

    for id in [1, 2, 3] {
        client
            .send(req(id, "control/keepAlive", serde_json::json!({})))
            .await
            .unwrap();
    }

    for id in [1, 2, 3] {
        let reply = client.recv().await.unwrap().unwrap();
        match reply {
            JsonRpcMessage::Response(r) => {
                assert_eq!(r.request_id, RequestId::Integer(id));
            }
            other => panic!("expected Response for id={id}, got {other:?}"),
        }
    }

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn app_server_bridge_entrypoint_dispatches_and_forwards_external_notifications() {
    #[derive(Debug, Clone)]
    struct TestHandle;

    let (server_end, client) = InMemoryTransport::pair(32);
    let (external_tx, external_rx) = tokio::sync::mpsc::channel(8);
    let sdk_server = SdkServer::new(server_end).with_external_notifications(external_rx);
    let app_server = std::sync::Arc::new(coco_app_server::AppServer::<TestHandle>::new(1, 8));
    let adapter = coco_app_server::JsonRpcAdapter::with_channel_capacity(app_server, 8);
    let connection = adapter.connect();
    let server_task =
        tokio::spawn(async move { sdk_server.run_app_server_connection(connection).await });

    client
        .send(req(7, "control/keepAlive", serde_json::json!({})))
        .await
        .unwrap();
    external_tx
        .send(CoreEvent::Protocol(
            ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            },
        ))
        .await
        .unwrap();

    let mut saw_response = false;
    let mut saw_notification = false;
    for _ in 0..2 {
        match client.recv().await.unwrap().unwrap() {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.request_id, RequestId::Integer(7));
                assert!(response.result.is_null());
                saw_response = true;
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, "session/stateChanged");
                saw_notification = true;
            }
            other => panic!("unexpected SDK message: {other:?}"),
        }
    }
    assert!(saw_response);
    assert!(saw_notification);

    drop(external_tx);
    drop(client);
    server_task
        .await
        .unwrap()
        .expect("AppServer bridge exits cleanly");
}

// ----- CoreEvent → JsonRpcNotification translation ----------------------

#[test]
fn core_event_protocol_serializes_to_notification() {
    use coco_types::ServerNotification;
    use coco_types::TurnStartedParams;

    let event = CoreEvent::Protocol(ServerNotification::TurnStarted(TurnStartedParams {
        turn_id: coco_types::TurnId::from("t1"),
    }));

    let notif = core_event_to_notification(event).expect("should translate");
    assert_eq!(notif.method, "turn/started");
    assert_eq!(notif.params["turn_id"], "t1");
}

#[test]
fn core_event_tui_is_dropped() {
    let event = CoreEvent::Tui(coco_types::TuiOnlyEvent::ToolCallDelta {
        call_id: "c1".into(),
        delta: "foo".into(),
    });
    assert!(core_event_to_notification(event).is_none());
}

#[test]
fn core_event_stream_returns_none_handled_by_accumulator() {
    // Stream events are handled by the writer task's StreamAccumulator,
    // not by core_event_to_notification. They return None here.
    use coco_types::AgentStreamEvent;
    let event = CoreEvent::Stream(AgentStreamEvent::TextDelta {
        turn_id: "t1".into(),
        delta: "hello".into(),
    });
    assert!(
        core_event_to_notification(event).is_none(),
        "Stream events should return None — handled by writer task accumulator"
    );
}
