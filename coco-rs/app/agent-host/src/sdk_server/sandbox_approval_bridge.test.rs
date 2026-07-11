use coco_sandbox::{
    SandboxApprovalBridge, SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
use std::sync::Arc;

use super::{
    SANDBOX_NETWORK_ACCESS_TOOL_NAME, SANDBOX_PATH_ACCESS_TOOL_NAME, SdkSandboxApprovalBridge,
};
use crate::sdk_server::handlers::SdkServerState;

#[tokio::test]
async fn test_synthetic_tool_names_match_ts() {
    // The wire string is part of the contract — SDK clients
    // pattern-match on it to surface the right approval dialog.
    assert_eq!(SANDBOX_NETWORK_ACCESS_TOOL_NAME, "SandboxNetworkAccess");
    assert_eq!(SANDBOX_PATH_ACCESS_TOOL_NAME, "SandboxPathAccess");
}

#[tokio::test]
async fn test_bridge_rejects_when_transport_uninitialised() {
    // Fail-closed: without an SDK transport published, the bridge
    // must reject — preserving the underlying sandbox deny error
    // rather than silently approving.
    let state = Arc::new(SdkServerState::default());
    let bridge = SdkSandboxApprovalBridge::new(state);
    let req = SandboxApprovalRequest {
        operation: SandboxOperation::Network,
        path: "api.example.com".into(),
        reason: "network access disabled".into(),
    };
    let decision = bridge.request_approval(req).await;
    assert_eq!(decision, SandboxApprovalDecision::Rejected);
}
