//! Dispatcher-level tests: routing, parsing, lifecycle, and the
//! CoreEvent → JsonRpcNotification translator.
//!
//! Per-handler behavior (session/*, turn/*, approval/*, control/*) is
//! tested in `handlers/tests.rs`.

use coco_types::JsonRpcMessage;
use coco_types::JsonRpcRequest;
use coco_types::RequestId;
use coco_types::error_codes;
use pretty_assertions::assert_eq;

use super::*;
use crate::sdk_server::InMemoryTransport;

fn req(id: i64, method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest {
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
    let handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    (handle, client_end)
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
            assert_eq!(e.code, error_codes::INVALID_PARAMS);
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
