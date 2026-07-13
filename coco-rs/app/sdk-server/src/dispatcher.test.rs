//! Dispatcher-level tests: routing, parsing, lifecycle, and the
//! CoreEvent → JsonRpcNotification translator.
//!
//! Per-method AppServer behavior is covered by the shared host handler
//! and multi-session tests.

use coco_types::JsonRpcMessage;
use coco_types::JsonRpcRequest;
use coco_types::RequestId;
use coco_types::ServerNotification;
use coco_types::error_codes;
use pretty_assertions::assert_eq;

use super::*;
use crate::InMemoryTransport;

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
    let bridge_host = coco_agent_host::remote_host::RemoteAppServerBridgeHost::ephemeral();
    let server = SdkServer::new(server_end, bridge_host);
    let handle = spawn_app_server_bridge(server);
    (handle, client_end)
}

fn spawn_app_server_bridge(server: SdkServer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let app_server = std::sync::Arc::new(coco_agent_host::remote_host::RemoteAppServer::new(
            /*max_sessions*/ 1, /*channel_capacity*/ 32,
        ));
        let adapter = coco_app_server::JsonRpcAdapter::with_channel_capacity(app_server, 32);
        let connection = adapter.connect();
        let _ = server.run_app_server_connection(connection).await;
    })
}

async fn initialize_connection(client: &InMemoryTransport) {
    client
        .send(req(
            0,
            "initialize",
            serde_json::to_value(coco_types::InitializeParams::default())
                .expect("serialize initialize params"),
        ))
        .await
        .expect("send initialize");
    let reply = client
        .recv()
        .await
        .expect("receive initialize")
        .expect("frame");
    match reply {
        JsonRpcMessage::Response(response) => {
            assert_eq!(response.request_id, RequestId::Integer(0));
        }
        other => panic!("expected initialize response, got {other:?}"),
    }
}

#[tokio::test]
async fn keep_alive_returns_empty_ok_response() {
    let (server_task, client) = spawn_server().await;
    initialize_connection(&client).await;

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
    initialize_connection(&client).await;

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
    initialize_connection(&client).await;
    // Immediately drop the client → server sees EOF → exits cleanly.
    drop(client);
    tokio::time::timeout(std::time::Duration::from_secs(2), server_task)
        .await
        .expect("server should exit on client drop")
        .expect("server task should not panic");
}

#[tokio::test]
async fn multiple_requests_are_processed_concurrently() {
    let (server_task, client) = spawn_server().await;
    initialize_connection(&client).await;

    for id in [1, 2, 3] {
        client
            .send(req(id, "control/keepAlive", serde_json::json!({})))
            .await
            .unwrap();
    }

    let mut response_ids = Vec::new();
    for _ in [1, 2, 3] {
        let reply = client.recv().await.unwrap().unwrap();
        match reply {
            JsonRpcMessage::Response(r) => {
                response_ids.push(r.request_id);
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }
    response_ids.sort_by_key(|id| match id {
        RequestId::Integer(id) => *id,
        RequestId::String(_) => i64::MAX,
    });
    assert_eq!(
        response_ids,
        vec![
            RequestId::Integer(1),
            RequestId::Integer(2),
            RequestId::Integer(3),
        ]
    );

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn app_server_bridge_entrypoint_dispatches_and_forwards_external_notifications() {
    let (server_end, client) = InMemoryTransport::pair(32);
    let (external_tx, external_rx) = tokio::sync::mpsc::channel(8);
    let bridge_host = coco_agent_host::remote_host::RemoteAppServerBridgeHost::ephemeral();
    let sdk_server =
        SdkServer::new(server_end, bridge_host).with_external_notifications(external_rx);
    let app_server = std::sync::Arc::new(coco_agent_host::remote_host::RemoteAppServer::new(1, 8));
    let adapter = coco_app_server::JsonRpcAdapter::with_channel_capacity(app_server, 8);
    let connection = adapter.connect();
    let server_task =
        tokio::spawn(async move { sdk_server.run_app_server_connection(connection).await });
    initialize_connection(&client).await;

    client
        .send(req(7, "control/keepAlive", serde_json::json!({})))
        .await
        .unwrap();
    external_tx
        .send(CoreEvent::Protocol(ServerNotification::PluginsChanged {
            reason: "test change".to_string(),
        }))
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
                assert_eq!(notification.method, "plugins/changed");
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
