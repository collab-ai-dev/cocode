//! Session-targeted SDK tool permission bridge.

use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::{
    ToolPermissionBridge, ToolPermissionDecision, ToolPermissionRequest, ToolPermissionResolution,
};
use coco_types::{ApprovalDecision, ServerAskForApprovalParams};
use tracing::{debug, warn};

pub struct SdkPermissionBridge {
    app_server: Arc<coco_app_server::AppServer<super::LocalAppSessionHandle>>,
    session: crate::session_runtime::SessionHandle,
}

impl SdkPermissionBridge {
    pub fn new(
        app_server: Arc<coco_app_server::AppServer<super::LocalAppSessionHandle>>,
        session: crate::session_runtime::SessionHandle,
    ) -> Self {
        Self {
            app_server,
            session,
        }
    }
}

#[async_trait]
impl ToolPermissionBridge for SdkPermissionBridge {
    async fn request_permission(
        &self,
        mut request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        crate::leader_permission::enrich_in_process_worker_badge(&mut request);
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
                .filter_map(|suggestion| serde_json::to_value(suggestion).ok())
                .collect(),
        };
        debug!(
            session_id = %self.session.session_id(),
            request_id = %request.id,
            tool = %request.tool_name,
            "asking targeted SDK surface for approval"
        );
        let title = format!("Permission request: {}", request.tool_name);
        self.session
            .fire_notification_hooks(
                "permission_prompt",
                "Coco needs your permission to use a tool",
                Some(&title),
            )
            .await;
        let reply = self
            .app_server
            .route_server_request_with_reply(
                self.session.session_id().clone(),
                coco_app_server::SurfaceCapability::Interactive,
                None,
                coco_types::ServerRequest::AskForApproval(params),
            )
            .map_err(|error| format!("route approval request failed: {error:?}"))?
            .await
            .map_err(|_| "approval reply channel closed".to_string())?;
        match reply {
            coco_app_server::ServerRequestReply::Approval(parsed) => {
                let approved = matches!(parsed.decision, ApprovalDecision::Allow);
                let decision = if approved {
                    ToolPermissionDecision::Approved
                } else {
                    ToolPermissionDecision::Rejected
                };
                let applied_updates = match (approved, parsed.permission_update) {
                    (true, Some(update)) => {
                        self.session
                            .apply_permission_updates_everywhere(std::slice::from_ref(&update))
                            .await;
                        vec![update]
                    }
                    _ => Vec::new(),
                };
                Ok(ToolPermissionResolution {
                    decision,
                    feedback: parsed.feedback,
                    applied_updates,
                    updated_input: parsed.updated_input,
                    content_blocks: parsed.content_blocks,
                    detail: None,
                })
            }
            coco_app_server::ServerRequestReply::Error(error) => {
                warn!(
                    session_id = %self.session.session_id(),
                    request_id = %request.id,
                    code = error.code,
                    message = %error.message,
                    "SDK client returned approval error"
                );
                Ok(ToolPermissionResolution {
                    decision: ToolPermissionDecision::Rejected,
                    feedback: Some(format!("approval error: {}", error.message)),
                    applied_updates: Vec::new(),
                    updated_input: None,
                    content_blocks: None,
                    detail: None,
                })
            }
            other => Err(format!("unexpected approval reply: {other:?}")),
        }
    }
}
