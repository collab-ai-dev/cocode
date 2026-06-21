//! Tests for `SdkPermissionBridge`.
//!
//! These verify the bridge correctly translates between the engine's
//! `ToolPermissionBridge` trait and the SDK's `approval/askForApproval`
//! / `approval/resolve` control messages by round-tripping requests
//! through an `InMemoryTransport`.

use std::sync::Arc;

use coco_tool_runtime::ToolPermissionBridge;
use coco_tool_runtime::ToolPermissionDecision;
use coco_tool_runtime::ToolPermissionRequest;
use coco_types::JsonRpcMessage;
use pretty_assertions::assert_eq;

use super::*;
use crate::sdk_server::InMemoryTransport;
use crate::sdk_server::SdkServer;
use crate::sdk_server::SdkTransport;

/// Build a bridge backed by a live `SdkServer` running on an
/// in-memory transport. Returns the server task, the client side of
/// the transport, and the bridge ready to consult.
async fn make_bridge() -> (
    tokio::task::JoinHandle<()>,
    Arc<InMemoryTransport>,
    SdkPermissionBridge,
) {
    let (server_end, client_end) = InMemoryTransport::pair(32);
    let server = SdkServer::new(server_end);
    let state = server.state();
    let handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let bridge = SdkPermissionBridge::new(state);
    (handle, client_end, bridge)
}

#[tokio::test]
async fn request_permission_approved_round_trip() {
    let (server_task, client, bridge) = make_bridge().await;

    // Kick off an approval request from the bridge side.
    let ask_task = tokio::spawn(async move {
        bridge
            .request_permission(ToolPermissionRequest {
                id: "req-1".into(),
                tool_use_id: "toolu_01AAA".into(),
                agent_id: "agent-main".into(),
                tool_name: "Bash".into(),
                description: "Execute ls".into(),
                input: serde_json::json!({ "command": "ls" }),
                cwd: None,
                suggestions: vec![],
                choices: None,
                detail: None,
                worker_badge: None,
            })
            .await
    });

    // Client side: read the inbound ServerRequest, reply with allow.
    let incoming = client.recv().await.unwrap().unwrap();
    let req_id = match incoming {
        JsonRpcMessage::Request(r) => {
            assert_eq!(r.method, "approval/askForApproval");
            assert_eq!(r.params["tool_name"], "Bash");
            assert_eq!(r.params["request_id"], "req-1");
            assert_eq!(r.params["tool_use_id"], "toolu_01AAA");
            assert_eq!(r.params["agent_id"], "agent-main");
            r.request_id
        }
        other => panic!("expected Request, got {other:?}"),
    };

    client
        .send(JsonRpcMessage::Response(coco_types::JsonRpcResponse {
            request_id: req_id,
            result: serde_json::json!({
                "request_id": "req-1",
                "decision": "allow",
                "feedback": "looks fine"
            }),
        }))
        .await
        .unwrap();

    // Bridge should now resolve with Approved.
    let resolution = ask_task.await.unwrap().expect("bridge returned Ok");
    assert_eq!(resolution.decision, ToolPermissionDecision::Approved);
    assert_eq!(resolution.feedback.as_deref(), Some("looks fine"));

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn request_permission_denied_round_trip() {
    let (server_task, client, bridge) = make_bridge().await;

    let ask_task = tokio::spawn(async move {
        bridge
            .request_permission(ToolPermissionRequest {
                id: "req-2".into(),
                tool_use_id: "toolu_01BBB".into(),
                agent_id: "agent-main".into(),
                tool_name: "Bash".into(),
                description: "rm -rf /".into(),
                input: serde_json::json!({ "command": "rm -rf /" }),
                cwd: None,
                suggestions: vec![],
                choices: None,
                detail: None,
                worker_badge: None,
            })
            .await
    });

    let incoming = client.recv().await.unwrap().unwrap();
    let req_id = match incoming {
        JsonRpcMessage::Request(r) => r.request_id,
        other => panic!("expected Request, got {other:?}"),
    };

    client
        .send(JsonRpcMessage::Response(coco_types::JsonRpcResponse {
            request_id: req_id,
            result: serde_json::json!({
                "request_id": "req-2",
                "decision": "deny",
                "feedback": "nope"
            }),
        }))
        .await
        .unwrap();

    let resolution = ask_task.await.unwrap().expect("Ok");
    assert_eq!(resolution.decision, ToolPermissionDecision::Rejected);
    assert_eq!(resolution.feedback.as_deref(), Some("nope"));

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn request_permission_client_error_is_treated_as_denial() {
    let (server_task, client, bridge) = make_bridge().await;

    let ask_task = tokio::spawn(async move {
        bridge
            .request_permission(ToolPermissionRequest {
                id: "req-3".into(),
                tool_use_id: "toolu_01CCC".into(),
                agent_id: "agent-main".into(),
                tool_name: "Bash".into(),
                description: "test".into(),
                input: serde_json::json!({}),
                cwd: None,
                suggestions: vec![],
                choices: None,
                detail: None,
                worker_badge: None,
            })
            .await
    });

    let incoming = client.recv().await.unwrap().unwrap();
    let req_id = match incoming {
        JsonRpcMessage::Request(r) => r.request_id,
        other => panic!("expected Request, got {other:?}"),
    };

    client
        .send(JsonRpcMessage::Error(coco_types::JsonRpcError {
            request_id: req_id,
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "client UI crashed".into(),
            data: None,
        }))
        .await
        .unwrap();

    let resolution = ask_task.await.unwrap().expect("Ok wrapping");
    assert_eq!(resolution.decision, ToolPermissionDecision::Rejected);
    let feedback = resolution.feedback.expect("feedback");
    assert!(feedback.contains("client UI crashed"));

    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn request_permission_errors_if_transport_not_initialized() {
    // Build a bare state with no transport and no SdkServer running.
    let state = Arc::new(SdkServerState::default());
    let bridge = SdkPermissionBridge::new(state);

    let result = bridge
        .request_permission(ToolPermissionRequest {
            id: "r".into(),
            tool_use_id: "toolu_01DDD".into(),
            agent_id: "a".into(),
            tool_name: "Bash".into(),
            description: "x".into(),
            input: serde_json::json!({}),
            cwd: None,
            suggestions: vec![],
            choices: None,
            detail: None,
            worker_badge: None,
        })
        .await;
    let err = result.expect_err("should error when transport missing");
    assert!(err.contains("transport not initialized"));
}
