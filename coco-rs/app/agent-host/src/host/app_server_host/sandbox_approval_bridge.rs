//! `SandboxApprovalBridge` impl that routes through AppServer.
//!
//! Sandbox network approvals are surfaced as a synthetic tool named
//! `SandboxNetworkAccess` so remote interactive surfaces see one uniform
//! permission protocol for both regular tools and sandbox-level operations. This
//! crate's [`coco_sandbox::SandboxApprovalBridge`] is the producer-side
//! seam (D7); this module is the AppServer bridge that connects it to the
//! existing `approval/askForApproval` round-trip already used by
//! [`crate::app_server_host::AppServerPermissionBridge`] for tool permissions.
//!
//! ## Wire shape
//!
//! Outbound: `ServerAskForApprovalParams { tool_name = "SandboxNetworkAccess",
//! input = { host, port?, path? }, description, ... }`.
//!
//! Inbound: same `ApprovalResolveParams { decision: Allow|Deny }`
//! response shape remote clients already implement.

use std::sync::Arc;

use async_trait::async_trait;
use coco_sandbox::{
    SandboxApprovalBridge, SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
use coco_types::{ApprovalDecision, ServerAskForApprovalParams};
use tracing::warn;
use uuid::Uuid;

/// Synthetic tool name surfaced to remote surfaces so sandbox approvals
/// reuse the regular tool-permission UI / handlers without a separate
/// message type.
pub const SANDBOX_NETWORK_ACCESS_TOOL_NAME: &str = "SandboxNetworkAccess";

/// Synthetic tool name for filesystem-level sandbox approvals
/// (path read / write). coco-rs has a stricter filesystem sandbox
/// and surfaces denied paths through the same channel so remote surfaces
/// can prompt with one consistent dialog.
pub const SANDBOX_PATH_ACCESS_TOOL_NAME: &str = "SandboxPathAccess";

/// AppServer-backed sandbox approval bridge.
///
/// The bridge is bound to the same session capability whose sandbox
/// produced the request. AppServer selects that session's interactive
/// surface and owns reply correlation.
pub struct AppServerSandboxApprovalBridge {
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    session: crate::session_runtime::SessionHandle,
}

impl AppServerSandboxApprovalBridge {
    pub fn new(
        app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        session: crate::session_runtime::SessionHandle,
    ) -> Self {
        Self {
            app_server,
            session,
        }
    }
}

#[async_trait]
impl SandboxApprovalBridge for AppServerSandboxApprovalBridge {
    async fn request_approval(&self, request: SandboxApprovalRequest) -> SandboxApprovalDecision {
        // `SandboxOperation` is `#[non_exhaustive]`; future kinds
        // (subprocess spawn, etc.) need an explicit wire mapping. We
        // route unknown kinds through the path-access tool with a
        // generic input shape so the remote surface at least sees the
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
        // Fire the Notification hook before blocking on the remote surface so
        // the same hook fires regardless of whether the prompt comes from
        // a regular tool or a sandbox-level deny. Best-effort — runtime
        // not yet installed (e.g. tests) leaves the hook unfired.
        let title = format!("Sandbox prompt: {tool_name}");
        self.session
            .fire_notification_hooks(
                "permission_prompt",
                "Coco needs your permission for a sandboxed operation",
                Some(&title),
            )
            .await;

        let reply = match self.app_server.route_server_request_with_reply(
            self.session.session_id().clone(),
            coco_app_server::SurfaceCapability::Interactive,
            self.session.active_turn_id(),
            coco_types::ServerRequest::AskForApproval(params),
        ) {
            Ok(receiver) => match receiver.await {
                Ok(reply) => reply,
                Err(_) => {
                    warn!(
                        session_id = %self.session.session_id(),
                        "AppServerSandboxApprovalBridge: reply channel closed; rejecting"
                    );
                    return SandboxApprovalDecision::Rejected;
                }
            },
            Err(e) => {
                warn!(
                    error = ?e,
                    session_id = %self.session.session_id(),
                    "AppServerSandboxApprovalBridge: route failed; rejecting"
                );
                return SandboxApprovalDecision::Rejected;
            }
        };

        match reply {
            coco_app_server::ServerRequestReply::Approval(parsed) => match parsed.decision {
                ApprovalDecision::Allow => SandboxApprovalDecision::Approved,
                ApprovalDecision::Deny => SandboxApprovalDecision::Rejected,
            },
            coco_app_server::ServerRequestReply::Error(e) => {
                warn!(
                    code = e.code,
                    message = %e.message,
                    "AppServerSandboxApprovalBridge: client returned error; rejecting"
                );
                SandboxApprovalDecision::Rejected
            }
            other => {
                warn!(
                    ?other,
                    "AppServerSandboxApprovalBridge: unexpected reply; rejecting"
                );
                SandboxApprovalDecision::Rejected
            }
        }
    }
}
