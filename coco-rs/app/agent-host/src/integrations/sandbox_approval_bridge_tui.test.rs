use std::sync::Arc;

use coco_sandbox::SandboxApprovalBridge;
use coco_sandbox::SandboxApprovalDecision;
use coco_sandbox::SandboxApprovalRequest;
use coco_sandbox::SandboxOperation;

use super::*;
use crate::tui_permission_bridge::{new_pending_map, send_resolution, take_pending};

fn network_request(host: &str) -> SandboxApprovalRequest {
    SandboxApprovalRequest {
        operation: SandboxOperation::Network,
        path: host.into(),
        reason: format!("network connection to {host}"),
    }
}

/// Drives `request_approval` to completion: emits the overlay event, then
/// resolves the shared pending entry with `approved` (mirroring the
/// tui_runner `ApprovalResponse` arm). Returns the bridge's decision.
async fn run_with_response(approved: bool) -> SandboxApprovalDecision {
    let pending = new_pending_map();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = Arc::new(TuiSandboxApprovalBridge::new(tx, pending.clone()));

    let b = bridge.clone();
    let handle = tokio::spawn(async move { b.request_approval(network_request("evil.com")).await });

    // The overlay event carries the fresh request_id.
    let request_id = match rx.recv().await.expect("overlay event emitted") {
        CoreEvent::Tui(TuiOnlyEvent::SandboxApprovalRequired {
            request_id,
            operation,
        }) => {
            assert!(operation.contains("evil.com"), "operation: {operation}");
            request_id
        }
        other => panic!("unexpected event: {other:?}"),
    };

    // Simulate the user's Approve/Deny via the shared pending map.
    let entry = take_pending(&pending, &request_id)
        .await
        .expect("pending entry registered");
    assert!(send_resolution(
        entry,
        approved,
        None,
        vec![],
        None,
        None,
        None
    ));

    handle.await.expect("request_approval task")
}

#[tokio::test]
async fn test_sandbox_bridge_approve_tunnels() {
    assert_eq!(
        run_with_response(true).await,
        SandboxApprovalDecision::Approved
    );
}

#[tokio::test]
async fn test_sandbox_bridge_deny_stays_rejected() {
    assert_eq!(
        run_with_response(false).await,
        SandboxApprovalDecision::Rejected
    );
}

#[tokio::test]
async fn test_sandbox_bridge_fail_closed_when_tui_gone() {
    // The notification receiver is dropped → the send fails → fail closed.
    let pending = new_pending_map();
    let (tx, rx) = mpsc::channel::<CoreEvent>(8);
    drop(rx);
    let bridge = TuiSandboxApprovalBridge::new(tx, pending);
    assert_eq!(
        bridge.request_approval(network_request("evil.com")).await,
        SandboxApprovalDecision::Rejected
    );
}
