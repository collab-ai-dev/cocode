//! `ToolPermissionBridge` implementation backed by the SDK server.
//!
//! When the agent hits a `PermissionDecision::Ask` during tool
//! execution, the engine calls `ctx.permission_bridge.request_permission`.
//! This module provides the bridge impl used in SDK mode: it issues a
//! `ServerRequest::AskForApproval` on the transport via
//! [`SdkServerState::send_server_request`] and translates the client's
//! `ApprovalResolveParams` reply back into [`ToolPermissionResolution`].
//!
//! The SDK client is the ultimate authority for any tool gated on
//! `ask` semantics.
//!
//! See `event-system-design.md` §6.

use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::ToolPermissionBridge;
use coco_tool_runtime::ToolPermissionDecision;
use coco_tool_runtime::ToolPermissionRequest;
use coco_tool_runtime::ToolPermissionResolution;
use coco_types::ApprovalDecision;
use coco_types::ApprovalResolveParams;
use coco_types::JsonRpcMessage;
use coco_types::ServerAskForApprovalParams;
use tracing::debug;
use tracing::warn;

use crate::sdk_server::handlers::SdkServerState;

/// Bridge the engine's permission gate to the SDK control protocol.
///
/// Holds a reference to the shared `SdkServerState` so it can:
/// - Read the cached transport handle
/// - Issue a `ServerRequest::AskForApproval` via `send_server_request`
/// - Translate the response into a `ToolPermissionResolution`
///
/// Construction is cheap — just an `Arc` clone. Install one per turn
/// on `QueryEngineConfig` / `ToolUseContext`.
pub struct SdkPermissionBridge {
    state: Arc<SdkServerState>,
}

impl SdkPermissionBridge {
    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ToolPermissionBridge for SdkPermissionBridge {
    async fn request_permission(
        &self,
        mut request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        // In-process teammates inherit the leader's bridge — badge them from
        // the live task-local identity (same as the TUI bridge).
        crate::leader_permission::enrich_in_process_worker_badge(&mut request);

        // Read the transport handle the dispatcher published at startup.
        let transport = {
            let guard = self.state.transport.read().await;
            match guard.as_ref() {
                Some(t) => t.clone(),
                None => {
                    return Err(
                        "SdkPermissionBridge: transport not initialized; SdkServer::run() \
                         must be running before the bridge is consulted"
                            .into(),
                    );
                }
            }
        };

        // Build the `approval/askForApproval` params via the typed struct so
        // the serde schema stays the single source of truth (any new optional
        // field shows up here automatically).
        let params = ServerAskForApprovalParams {
            request_id: request.id.clone(),
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            tool_use_id: request.tool_use_id.clone(),
            description: Some(request.description.clone()),
            title: None,
            display_name: None,
            blocked_path: None,
            decision_reason: None,
            agent_id: Some(request.agent_id.clone()),
            cwd: request.cwd.clone(),
            permission_suggestions: request
                .suggestions
                .iter()
                .filter_map(|s| serde_json::to_value(s).ok())
                .collect(),
        };
        let params = serde_json::to_value(&params)
            .map_err(|e| format!("serialize ServerAskForApprovalParams: {e}"))?;

        debug!(
            request_id = %request.id,
            tool = %request.tool_name,
            "SdkPermissionBridge: asking client for approval"
        );

        // Fire the Notification hook before blocking on the client's reply.
        // Best-effort — a runtime not yet installed (e.g. tests) leaves
        // the hook unfired.
        if let Some(runtime) = self.state.session_runtime.read().await.clone() {
            let title = format!("Permission request: {}", request.tool_name);
            runtime
                .fire_notification_hooks(
                    "permission_prompt",
                    "Claude Code needs your permission to use a tool",
                    Some(&title),
                )
                .await;
        }

        // Issue the outbound ServerRequest and await the reply.
        let reply = self
            .state
            .send_server_request(&transport, "approval/askForApproval", params)
            .await
            .map_err(|e| format!("send_server_request failed: {e}"))?;

        // Interpret the reply — parse the `ApprovalResolveParams` shape.
        match reply {
            JsonRpcMessage::Response(r) => {
                let parsed: ApprovalResolveParams = serde_json::from_value(r.result)
                    .map_err(|e| format!("invalid approval response: {e}"))?;
                let approved = matches!(parsed.decision, ApprovalDecision::Allow);
                let decision = if approved {
                    ToolPermissionDecision::Approved
                } else {
                    ToolPermissionDecision::Rejected
                };
                // Apply any rule the SDK client authorized ("always allow")
                // through the SAME unified entry the TUI dialog uses (base
                // config + live overlay + disk). The live overlay write is
                // what gives in-cycle parity with TUI: the approved rule takes
                // effect for the remaining tool calls of THIS cycle, not just
                // the next build. `applied_updates` echoes it for audit.
                let applied_updates = match (approved, parsed.permission_update) {
                    (true, Some(update)) => {
                        match self.state.session_runtime.read().await.clone() {
                            Some(runtime) => {
                                runtime
                                    .apply_permission_updates_everywhere(std::slice::from_ref(
                                        &update,
                                    ))
                                    .await;
                            }
                            None => warn!(
                                request_id = %request.id,
                                "SDK approval carried permission_update but session_runtime \
                                 is not installed; rule not applied"
                            ),
                        }
                        vec![update]
                    }
                    _ => Vec::new(),
                };
                Ok(ToolPermissionResolution {
                    decision,
                    feedback: parsed.feedback,
                    applied_updates,
                    // The protocol carries updated input on
                    // `ApprovalResolveParams`; SDK clients ship
                    // `AskUserQuestion` answers (and any other pre-tool
                    // input rewrite) here.
                    updated_input: parsed.updated_input,
                    // Image attachments pasted alongside the answer ride
                    // this slot.
                    content_blocks: parsed.content_blocks,
                    detail: None,
                })
            }
            JsonRpcMessage::Error(e) => {
                warn!(
                    request_id = %request.id,
                    code = e.code,
                    message = %e.message,
                    "SdkPermissionBridge: client returned error for approval ask"
                );
                // Client-side error on the approval path is treated as a
                // rejection — safer default than blocking execution.
                Ok(ToolPermissionResolution {
                    decision: ToolPermissionDecision::Rejected,
                    feedback: Some(format!("approval error: {}", e.message)),
                    applied_updates: Vec::new(),
                    updated_input: None,
                    content_blocks: None,
                    detail: None,
                })
            }
            other => Err(format!(
                "unexpected reply variant for approval ask: {other:?}"
            )),
        }
    }
}

#[cfg(test)]
#[path = "approval_bridge.test.rs"]
mod tests;
