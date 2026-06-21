//! Cross-process worker permission via mailbox file IPC.
//!
//! Extracted from `runner_loop.rs` (P1 split).
//!
//! Pane-mode teammates run in a *separate process* from the leader,
//! so they can't share the leader's `ToolPermissionBridge` directly.
//! Instead they enqueue a [`mailbox::ProtocolMessage::PermissionRequest`]
//! into the team-lead inbox, then poll their own inbox for the
//! matching [`mailbox::ProtocolMessage::PermissionResponse`].
//!
//! In-process teammates take the faster path of inheriting the
//! leader's bridge through `wire_engine`; this module is reserved
//! for the cross-process case.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::constants::TEAM_LEAD_NAME;
use crate::mailbox;
use crate::types::TeammateIdentity;

/// Poll interval for inbox scanning (ms). Mirrors [`crate::runner_loop::POLL_INTERVAL_MS`].
const POLL_INTERVAL_MS: u64 = 500;

/// Hard ceiling on how long a worker blocks waiting for the leader to
/// resolve a permission request. Without it a leader that never answers
/// (crashed, no poller, denied silently) would hang the worker forever.
/// On expiry the bridge fails closed (treated as a denial), which is the
/// safe direction for an unanswered approval prompt.
const PERMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default)]
pub struct MailboxPermissionOutcome {
    pub approved: bool,
    pub feedback: Option<String>,
    pub updated_input: Option<serde_json::Value>,
    pub permission_updates: Vec<coco_types::PermissionUpdate>,
}

/// Worker-side permission resolution: send a permission request to
/// the leader's mailbox and block on the matching
/// [`mailbox::ProtocolMessage::PermissionResponse`].
///
/// Returns `Some((approved, feedback))` on resolution, `None` on
/// cancellation or write failure.
pub async fn request_permission_via_mailbox(
    identity: &TeammateIdentity,
    cancelled: &AtomicBool,
    request: &coco_tool_runtime::ToolPermissionRequest,
) -> Option<MailboxPermissionOutcome> {
    let request_id = request.id.as_str();
    let tool_name = request.tool_name.as_str();
    let agent_id = format!("{}@{}", identity.agent_name, identity.team_name);
    let envelope = mailbox::create_permission_request_message(
        request_id,
        &agent_id,
        tool_name,
        &request.tool_use_id,
        &request.description,
        &request.input,
        request.cwd.as_deref(),
    );
    let message = mailbox::TeammateMessage {
        from: identity.agent_name.clone(),
        text: envelope,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: identity.color.as_ref().map(|c| c.as_str().to_string()),
        summary: Some("permission request".to_string()),
    };
    if mailbox::write_to_mailbox(TEAM_LEAD_NAME, message, &identity.team_name).is_err() {
        return None;
    }

    let deadline = tokio::time::Instant::now() + PERMISSION_WAIT_TIMEOUT;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                request_id,
                tool_name,
                "team permission: leader did not respond within {}s; failing closed",
                PERMISSION_WAIT_TIMEOUT.as_secs(),
            );
            return None;
        }
        let messages =
            mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();
        for (i, msg) in messages.iter().enumerate() {
            if msg.read {
                continue;
            }
            if !mailbox::is_structured_protocol_message(&msg.text) {
                continue;
            }
            if let Some(mailbox::ProtocolMessage::PermissionResponse {
                request_id: rid,
                subtype,
                response,
                error,
            }) = mailbox::parse_protocol_message(&msg.text)
                && rid == request_id
            {
                let _ = mailbox::mark_message_as_read_by_index(
                    &identity.agent_name,
                    &identity.team_name,
                    i,
                );
                let approved = subtype.is_success();
                let (updated_input, permission_updates) = response.unwrap_or_default().into_parts();
                return Some(MailboxPermissionOutcome {
                    approved,
                    feedback: error,
                    updated_input,
                    permission_updates,
                });
            }
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

/// `ToolPermissionBridge` impl backed by [`request_permission_via_mailbox`].
///
/// Install on a cross-process pane teammate's `ToolUseContext.permission_bridge`
/// so the deny site forwards approval prompts to the leader via
/// mailbox IPC. In-process teammates inherit the leader's bridge
/// directly via `wire_engine` and don't need this adapter.
pub struct MailboxPermissionBridge {
    identity: TeammateIdentity,
    cancelled: Arc<AtomicBool>,
}

impl MailboxPermissionBridge {
    pub fn new(identity: TeammateIdentity, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            identity,
            cancelled,
        }
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::ToolPermissionBridge for MailboxPermissionBridge {
    async fn request_permission(
        &self,
        request: coco_tool_runtime::ToolPermissionRequest,
    ) -> Result<coco_tool_runtime::ToolPermissionResolution, String> {
        let outcome =
            request_permission_via_mailbox(&self.identity, &self.cancelled, &request).await;
        match outcome {
            Some(outcome) if outcome.approved => Ok(coco_tool_runtime::ToolPermissionResolution {
                decision: coco_tool_runtime::ToolPermissionDecision::Approved,
                feedback: None,
                applied_updates: outcome.permission_updates,
                updated_input: outcome.updated_input,
                content_blocks: None,
                detail: None,
            }),
            Some(outcome) => Ok(coco_tool_runtime::ToolPermissionResolution {
                decision: coco_tool_runtime::ToolPermissionDecision::Rejected,
                feedback: outcome.feedback,
                applied_updates: outcome.permission_updates,
                updated_input: outcome.updated_input,
                content_blocks: None,
                detail: None,
            }),
            None => Err("Permission request cancelled or leader mailbox unreachable".into()),
        }
    }
}
