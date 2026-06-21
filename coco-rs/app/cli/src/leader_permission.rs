//! Leader-side resolution of cross-process teammate permission requests.
//!
//! A pane teammate (separate process) that hits a deny-path tool forwards
//! the approval prompt to the leader's inbox via mailbox IPC (see
//! [`coco_coordinator::MailboxPermissionBridge`]). The leader's per-turn
//! inbox poll ([`coco_query`]'s plan-mode reminder) hands each such request
//! to the setter registered here; we route it to the leader's human approval
//! UI (the session's `TuiPermissionBridge`) and write the decision back to
//! the worker's inbox.

use std::sync::Arc;

use coco_coordinator::mailbox;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolPermissionDecision;
use coco_tool_runtime::ToolPermissionRequest;

/// Register the leader's permission-queue setter, backed by `bridge` (the
/// session's `TuiPermissionBridge`). The coordinator holds one global setter;
/// call once for a leader session when agent-teams is enabled.
pub async fn register(bridge: ToolPermissionBridgeRef) {
    let setter: coco_coordinator::teammate::PermissionQueueSetter = Arc::new(move |value| {
        let bridge = bridge.clone();
        tokio::spawn(async move { handle_request(bridge, value).await });
    });
    coco_coordinator::teammate::register_leader_permission_queue(setter).await;
}

/// Enrich an in-process teammate's permission request with a worker badge.
///
/// In-process teammates inherit the LEADER's permission bridge, so their
/// requests reach the bridge with `worker_badge: None` — the generic
/// `coco_query` permission controller can't see the coordinator's task-local
/// identity. The bridge runs inline within the teammate's
/// `run_with_teammate_context` scope, so the live identity resolves here: the
/// in-process analog of the cross-process badge set in [`handle_request`].
/// No-op for the leader's own requests (not a teammate) and for requests that
/// already carry a badge.
pub fn enrich_in_process_worker_badge(request: &mut ToolPermissionRequest) {
    use coco_coordinator::identity;
    if request.worker_badge.is_some() || !identity::is_in_process_teammate() {
        return;
    }
    let Some(name) = identity::get_agent_name() else {
        return;
    };
    // Color cache is keyed on the `name@team` agent id (see coordinator's
    // `assign_teammate_color`), matching the cross-process lookup above.
    let color_key = identity::get_agent_id().unwrap_or_else(|| name.clone());
    request.worker_badge = Some(coco_types::WorkerBadge {
        name,
        color: coco_coordinator::pane::layout::get_teammate_color(&color_key)
            .unwrap_or(coco_types::AgentColorName::Cyan),
    });
}

/// Prompt the leader (human) for a worker's permission request, then write
/// the decision back to the worker's inbox. On any failure we write nothing,
/// leaving the worker's bounded wait to fail closed.
async fn handle_request(bridge: ToolPermissionBridgeRef, value: serde_json::Value) {
    let Ok(mailbox::ProtocolMessage::PermissionRequest {
        request_id,
        agent_id,
        tool_name,
        tool_use_id,
        description,
        input,
        cwd,
        ..
    }) = serde_json::from_value::<mailbox::ProtocolMessage>(value)
    else {
        return;
    };
    // The worker's agent_id is `worker_name@team_name`.
    let Some((worker_name, team_name)) = agent_id.rsplit_once('@') else {
        tracing::warn!(%agent_id, "leader permission: malformed worker agent id");
        return;
    };

    let req = ToolPermissionRequest {
        id: request_id.clone(),
        tool_use_id,
        agent_id: agent_id.clone(),
        tool_name,
        description,
        input,
        // The worker's own tool cwd (its directory, not the leader's), so the
        // leader's prompt resolves the worker's relative paths correctly.
        cwd,
        suggestions: Vec::new(),
        choices: None,
        detail: None,
        // Badge the worker so the leader sees who is asking. Color is the
        // worker's assigned per-teammate palette entry; fall back to Cyan
        // when unassigned.
        worker_badge: Some(coco_types::WorkerBadge {
            name: worker_name.to_string(),
            color: coco_coordinator::pane::layout::get_teammate_color(&agent_id)
                .unwrap_or(coco_types::AgentColorName::Cyan),
        }),
    };
    let resolution = match bridge.request_permission(req).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %request_id, "leader permission: bridge declined to resolve");
            return;
        }
    };
    let approved = matches!(resolution.decision, ToolPermissionDecision::Approved);
    if let Err(e) = mailbox::send_permission_response_via_mailbox(
        worker_name,
        &request_id,
        approved,
        resolution.feedback.as_deref(),
        resolution.updated_input,
        resolution.applied_updates,
        team_name,
    ) {
        tracing::warn!(error = %e, %request_id, "leader permission: failed to reply to worker");
    }
}
