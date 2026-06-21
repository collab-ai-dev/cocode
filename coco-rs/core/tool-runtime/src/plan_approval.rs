//! Plan-approval protocol messages exchanged between teammates and team
//! leads via the mailbox. Producers:
//! - teammate `ExitPlanModeTool` serializes [`PlanApprovalRequest`] and
//!   writes it to the lead's inbox.
//! - leader's `SendMessage` reply serializes [`PlanApprovalResponse`]
//!   into the teammate's inbox.
//!
//! Consumers (`PlanModeReminder::poll_teammate_approval` +
//! `inject_leader_pending_approvals`) deserialize back to these typed
//! structs — avoids string-keyed `serde_json::Value` access on internal
//! coco↔coco payloads per CLAUDE.md "Typed Structs over JSON Values".
//!
//! Wire format uses explicit field renames so both camelCase and
//! snake_case messages round-trip cleanly.

use coco_types::PermissionMode;
use serde::Deserialize;
use serde::Serialize;

/// Either side of the plan-approval handshake. Tagged on `type` to
/// tagged on `type` with `"plan_approval_request"` / `"plan_approval_response"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanApprovalMessage {
    PlanApprovalRequest(PlanApprovalRequest),
    PlanApprovalResponse(PlanApprovalResponse),
}

/// Teammate → lead: "Here is my finished plan. Please approve."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApprovalRequest {
    /// Teammate agent name.
    pub from: String,
    /// ISO-8601 timestamp of the submission. Optional for backward
    /// compatibility with older mailbox payloads and test fixtures.
    #[serde(default)]
    pub timestamp: String,
    /// Absolute path to the plan file on the teammate's side.
    #[serde(rename = "planFilePath", alias = "plan_file_path")]
    pub plan_file_path: String,
    /// Full plan text — the lead reads this, not the file, because the
    /// lead runs in a separate process without access to the teammate's
    /// plans dir.
    #[serde(rename = "planContent", alias = "plan_content")]
    pub plan_content: String,
    /// Correlation ID. Teammate's `awaiting_plan_approval_request_id`
    /// matches against this in the response.
    #[serde(rename = "requestId", alias = "request_id")]
    pub request_id: String,
}

/// Lead → teammate: "Approved / rejected. Here's what to do next."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApprovalResponse {
    /// Correlation ID echoing the request.
    #[serde(rename = "requestId", alias = "request_id")]
    pub request_id: String,
    /// True = approved (teammate may implement); false = rejected.
    pub approved: bool,
    /// Optional rejection rationale — surfaced verbatim in the teammate's
    /// next-turn reminder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Optional target mode the lead wants the teammate to switch into
    /// after approval (e.g. `accept_edits` so the teammate can proceed
    /// without per-edit prompts). None = stay in Plan.
    #[serde(
        rename = "permissionMode",
        alias = "permission_mode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<PermissionMode>,
}

#[cfg(test)]
#[path = "plan_approval.test.rs"]
mod tests;
