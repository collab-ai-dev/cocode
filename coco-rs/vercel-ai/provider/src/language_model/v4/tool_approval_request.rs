//! Language model V4 tool approval request type.
//!
//! Tool approval request emitted by a provider for a provider-executed tool call.

use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// Tool approval request emitted by a provider for a provider-executed tool call.
///
/// This is used for flows where the provider executes the tool (e.g. MCP tools)
/// but requires an explicit user approval before continuing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageModelV4ToolApprovalRequest {
    /// ID of the approval request. This ID is referenced by the subsequent
    /// tool-approval-response (tool message) to approve or deny execution.
    pub approval_id: String,
    /// The tool call ID that this approval request is for.
    pub tool_call_id: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl LanguageModelV4ToolApprovalRequest {
    /// Create a new tool approval request.
    pub fn new(approval_id: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            approval_id: approval_id.into(),
            tool_call_id: tool_call_id.into(),
            provider_metadata: None,
        }
    }

    /// Set provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

#[cfg(test)]
#[path = "tool_approval_request.test.rs"]
mod tests;
