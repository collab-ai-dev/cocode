//! Tool approval response type.

use serde::Deserialize;
use serde::Serialize;

/// Tool approval response prompt part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolApprovalResponse {
    /// ID of the tool approval.
    pub approval_id: String,
    /// Flag indicating whether the approval was granted or denied.
    pub approved: bool,
    /// Optional reason for the approval or denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Flag indicating whether the tool call is provider-executed.
    ///
    /// Only provider-executed tool approval responses should be sent to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
}

impl ToolApprovalResponse {
    /// Create a new tool approval response.
    pub fn new(approval_id: impl Into<String>, approved: bool) -> Self {
        Self {
            approval_id: approval_id.into(),
            approved,
            reason: None,
            provider_executed: None,
        }
    }

    /// Create an approved response.
    pub fn approved(approval_id: impl Into<String>) -> Self {
        Self::new(approval_id, true)
    }

    /// Create a denied response.
    pub fn denied(approval_id: impl Into<String>) -> Self {
        Self::new(approval_id, false)
    }

    /// Add a reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Mark as provider-executed.
    pub fn with_provider_executed(mut self) -> Self {
        self.provider_executed = Some(true);
        self
    }
}
