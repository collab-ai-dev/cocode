//! Provider-hosted tool approval bridge.
//!
//! Provider approval requests are assistant content, not local tool calls.
//! This module translates them onto the existing interactive permission
//! boundary and emits the tool-role approval response required to resume the
//! provider state machine.

use coco_llm_types::ToolApprovalRequestPart;
use coco_messages::Message;
use coco_tool_runtime::ToolPermissionDecision;
use coco_tool_runtime::ToolPermissionRequest;
use uuid::Uuid;

use crate::engine::QueryEngine;

impl QueryEngine {
    pub(crate) async fn resolve_provider_approvals(
        &self,
        approvals: &[ToolApprovalRequestPart],
        source_assistant_uuid: Uuid,
    ) -> Vec<Message> {
        let mut responses = Vec::with_capacity(approvals.len());
        for approval in approvals {
            let tool_name = approval
                .tool_name
                .as_deref()
                .unwrap_or("provider_hosted_tool");
            let (approved, reason) = self.resolve_provider_approval(approval, tool_name).await;
            responses.push(coco_messages::create_tool_approval_response_message(
                &approval.approval_id,
                &approval.tool_call_id,
                tool_name,
                approved,
                reason,
                Some(source_assistant_uuid),
            ));
        }
        responses
    }

    async fn resolve_provider_approval(
        &self,
        approval: &ToolApprovalRequestPart,
        tool_name: &str,
    ) -> (bool, Option<String>) {
        let Some(bridge) = self.permission_bridge.as_ref() else {
            return (
                false,
                Some("Interactive approval is unavailable".to_string()),
            );
        };
        let description = approval
            .context
            .clone()
            .unwrap_or_else(|| format!("Allow provider-hosted tool `{tool_name}` to run?"));
        let request = ToolPermissionRequest {
            id: format!("provider-approval-{}", Uuid::new_v4()),
            tool_use_id: approval.tool_call_id.clone(),
            agent_id: self.session_id.to_string(),
            tool_name: tool_name.to_string(),
            description,
            input: serde_json::json!({
                "approvalId": approval.approval_id,
                "context": approval.context,
                "arguments": approval.input,
            }),
            cwd: Some(self.config.workspace_cwd().to_string_lossy().into_owned()),
            suggestions: Vec::new(),
            choices: None,
            detail: None,
            worker_badge: None,
        };
        let resolution = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                return (false, Some("Turn cancelled while waiting for provider approval".into()));
            }
            result = bridge.request_permission(request) => result,
        };
        match resolution {
            Ok(resolution) => match resolution.decision {
                ToolPermissionDecision::Approved => (true, resolution.feedback),
                ToolPermissionDecision::Rejected => (
                    false,
                    Some(
                        resolution
                            .feedback
                            .unwrap_or_else(|| "Permission denied by client".into()),
                    ),
                ),
                ToolPermissionDecision::Aborted => (
                    false,
                    Some(
                        resolution
                            .feedback
                            .unwrap_or_else(|| "Permission request was aborted".into()),
                    ),
                ),
            },
            Err(error) => (false, Some(format!("Permission bridge failed: {error}"))),
        }
    }
}
