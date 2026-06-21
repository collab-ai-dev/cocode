//! `SandboxApprovalBridge` impl that routes through the SDK control
//! channel.
//!
//! Sandbox network approvals are surfaced as a synthetic tool named
//! `SandboxNetworkAccess` so SDK clients see one uniform permission
//! protocol for both regular tools and sandbox-level operations. This
//! crate's [`coco_sandbox::SandboxApprovalBridge`] is the producer-side
//! seam (D7); this module is the SDK adapter that connects it to the
//! existing `approval/askForApproval` round-trip already used by
//! [`crate::sdk_server::SdkPermissionBridge`] for tool permissions.
//!
//! ## Wire shape
//!
//! Outbound: `ServerAskForApprovalParams { tool_name = "SandboxNetworkAccess",
//! input = { host, port?, path? }, description, ... }`.
//!
//! Inbound: same `ApprovalResolveParams { decision: Allow|Deny }`
//! response shape SDK clients already implement.

use std::sync::Arc;

use async_trait::async_trait;
use coco_sandbox::{
    SandboxApprovalBridge, SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
use coco_types::{
    ApprovalDecision, ApprovalResolveParams, JsonRpcMessage, ServerAskForApprovalParams,
};
use tracing::warn;
use uuid::Uuid;

use crate::sdk_server::handlers::SdkServerState;

/// Synthetic tool name surfaced to SDK clients so sandbox approvals
/// reuse the regular tool-permission UI / handlers without a separate
/// message type.
pub const SANDBOX_NETWORK_ACCESS_TOOL_NAME: &str = "SandboxNetworkAccess";

/// Synthetic tool name for filesystem-level sandbox approvals
/// (path read / write). coco-rs has a stricter filesystem sandbox
/// and surfaces denied paths through the same channel so SDK clients
/// can prompt with one consistent dialog.
pub const SANDBOX_PATH_ACCESS_TOOL_NAME: &str = "SandboxPathAccess";

/// SDK-backed sandbox approval bridge.
///
/// Construction is cheap — just an `Arc<SdkServerState>` clone. The
/// SDK transport must be initialised (`SdkServer::run`) before the
/// first sandbox deny lands; otherwise we surface
/// `SandboxApprovalDecision::Rejected` so the underlying deny error
/// stands. Fail-closed by construction.
pub struct SdkSandboxApprovalBridge {
    state: Arc<SdkServerState>,
}

impl SdkSandboxApprovalBridge {
    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SandboxApprovalBridge for SdkSandboxApprovalBridge {
    async fn request_approval(&self, request: SandboxApprovalRequest) -> SandboxApprovalDecision {
        // Read the transport handle the dispatcher published at
        // startup. Missing transport → Rejected (preserves the
        // sandbox's deny error). Same fail-closed semantics
        // `SdkPermissionBridge` uses.
        let transport = {
            let guard = self.state.transport.read().await;
            match guard.as_ref() {
                Some(t) => t.clone(),
                None => {
                    warn!(
                        operation = request.operation.as_str(),
                        path = %request.path,
                        "SdkSandboxApprovalBridge: SDK transport unavailable; rejecting"
                    );
                    return SandboxApprovalDecision::Rejected;
                }
            }
        };

        // `SandboxOperation` is `#[non_exhaustive]`; future kinds
        // (subprocess spawn, etc.) need an explicit wire mapping. We
        // route unknown kinds through the path-access tool with a
        // generic input shape so the SDK client at least sees the
        // approval prompt — the alternative would be silent acceptance
        // or hard panic, both worse for the security model.
        let tool_name = match request.operation {
            SandboxOperation::Network => SANDBOX_NETWORK_ACCESS_TOOL_NAME,
            SandboxOperation::Read | SandboxOperation::Write => SANDBOX_PATH_ACCESS_TOOL_NAME,
            _ => SANDBOX_PATH_ACCESS_TOOL_NAME,
        };
        let input = match request.operation {
            SandboxOperation::Network => serde_json::json!({ "host": request.path }),
            SandboxOperation::Read => serde_json::json!({ "path": request.path, "write": false }),
            SandboxOperation::Write => serde_json::json!({ "path": request.path, "write": true }),
            _ => serde_json::json!({
                "path": request.path,
                "operation": request.operation.as_str(),
            }),
        };

        let params = ServerAskForApprovalParams {
            request_id: Uuid::new_v4().to_string(),
            tool_name: tool_name.into(),
            input,
            tool_use_id: Uuid::new_v4().to_string(),
            description: Some(format!(
                "Sandbox {} operation: {}",
                request.operation.as_str(),
                if request.path.is_empty() {
                    "(no path)"
                } else {
                    request.path.as_str()
                }
            )),
            title: None,
            display_name: None,
            blocked_path: if request.path.is_empty() {
                None
            } else {
                Some(request.path.clone())
            },
            decision_reason: Some(request.reason.clone()),
            agent_id: None,
            cwd: None,
            permission_suggestions: Vec::new(),
        };
        let params = match serde_json::to_value(&params) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "SdkSandboxApprovalBridge: failed to serialise params");
                return SandboxApprovalDecision::Rejected;
            }
        };

        // Fire the Notification hook before blocking on the SDK client so
        // the same hook fires regardless of whether the prompt comes from
        // a regular tool or a sandbox-level deny. Best-effort — runtime
        // not yet installed (e.g. tests) leaves the hook unfired.
        if let Some(runtime) = self.state.session_runtime.read().await.clone() {
            let title = format!("Sandbox prompt: {tool_name}");
            runtime
                .fire_notification_hooks(
                    "permission_prompt",
                    "Claude Code needs your permission for a sandboxed operation",
                    Some(&title),
                )
                .await;
        }

        let reply = match self
            .state
            .send_server_request(&transport, "approval/askForApproval", params)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    error = %e,
                    "SdkSandboxApprovalBridge: send_server_request failed; rejecting"
                );
                return SandboxApprovalDecision::Rejected;
            }
        };

        match reply {
            JsonRpcMessage::Response(r) => {
                let parsed: ApprovalResolveParams = match serde_json::from_value(r.result) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(error = %e, "SdkSandboxApprovalBridge: invalid response shape");
                        return SandboxApprovalDecision::Rejected;
                    }
                };
                match parsed.decision {
                    ApprovalDecision::Allow => SandboxApprovalDecision::Approved,
                    ApprovalDecision::Deny => SandboxApprovalDecision::Rejected,
                }
            }
            JsonRpcMessage::Error(e) => {
                warn!(
                    code = e.code,
                    message = %e.message,
                    "SdkSandboxApprovalBridge: client returned error; rejecting"
                );
                SandboxApprovalDecision::Rejected
            }
            other => {
                warn!(?other, "SdkSandboxApprovalBridge: unexpected reply variant");
                SandboxApprovalDecision::Rejected
            }
        }
    }
}

#[cfg(test)]
#[path = "sandbox_approval_bridge.test.rs"]
mod tests;
