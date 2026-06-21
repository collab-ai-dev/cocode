//! Tool approval request type.

use serde::Deserialize;
use serde::Serialize;

/// Tool approval request prompt part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolApprovalRequest {
    /// ID of the tool approval.
    pub approval_id: String,
    /// ID of the tool call that the approval request is for.
    pub tool_call_id: String,
}

impl ToolApprovalRequest {
    /// Create a new tool approval request.
    pub fn new(approval_id: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            approval_id: approval_id.into(),
            tool_call_id: tool_call_id.into(),
        }
    }
}
