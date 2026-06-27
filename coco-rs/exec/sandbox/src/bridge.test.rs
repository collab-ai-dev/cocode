use std::sync::Arc;
use tokio::sync::Mutex;

use super::*;

#[tokio::test]
async fn test_no_op_bridge_always_rejects() {
    let bridge = NoOpSandboxApprovalBridge;
    let request = SandboxApprovalRequest {
        operation: SandboxOperation::Write,
        path: "/etc/passwd".into(),
        reason: "outside writable roots".into(),
    };
    assert_eq!(
        bridge.request_approval(request).await,
        SandboxApprovalDecision::Rejected
    );
}

#[test]
fn test_sandbox_operation_as_str() {
    assert_eq!(SandboxOperation::Read.as_str(), "read");
    assert_eq!(SandboxOperation::Write.as_str(), "write");
    assert_eq!(SandboxOperation::Network.as_str(), "network");
}

/// Stub bridge that records every request for assertion.
pub(crate) struct RecordingBridge {
    pub decision: SandboxApprovalDecision,
    pub seen: Mutex<Vec<SandboxApprovalRequest>>,
}

#[async_trait::async_trait]
impl SandboxApprovalBridge for RecordingBridge {
    async fn request_approval(&self, request: SandboxApprovalRequest) -> SandboxApprovalDecision {
        self.seen.lock().await.push(request);
        self.decision
    }
}

#[tokio::test]
async fn test_recording_bridge_captures_requests() {
    let bridge = Arc::new(RecordingBridge {
        decision: SandboxApprovalDecision::Approved,
        seen: Mutex::new(Vec::new()),
    });
    let req = SandboxApprovalRequest {
        operation: SandboxOperation::Network,
        path: String::new(),
        reason: "network access disabled".into(),
    };
    assert_eq!(
        bridge.request_approval(req.clone()).await,
        SandboxApprovalDecision::Approved
    );
    let seen = bridge.seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].operation, SandboxOperation::Network);
}
