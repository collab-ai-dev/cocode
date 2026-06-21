//! Tool model message type.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ToolResultPart;

/// A tool message.
///
/// It contains the result of one or more tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolModelMessage {
    /// The role, always "tool".
    pub role: String,
    /// The message content.
    pub content: ToolContent,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl ToolModelMessage {
    /// Create a new tool message with a single result.
    pub fn single(result: ToolResultPart) -> Self {
        Self {
            role: "tool".to_string(),
            content: ToolContent::new(vec![ToolContentPart::ToolResult(result)]),
            provider_options: None,
        }
    }

    /// Create a new tool message with multiple results.
    pub fn parts(parts: Vec<ToolContentPart>) -> Self {
        Self {
            role: "tool".to_string(),
            content: ToolContent::new(parts),
            provider_options: None,
        }
    }

    /// Add provider options.
    pub fn with_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

/// Content of a tool message.
///
/// It is an array of tool result parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContent(pub Vec<ToolContentPart>);

/// A part of tool content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToolContentPart {
    /// A tool result.
    ToolResult(ToolResultPart),
    /// A tool approval response.
    ToolApprovalResponse(ToolApprovalResponse),
}

impl ToolContent {
    /// Create tool content from parts.
    pub fn new(parts: Vec<ToolContentPart>) -> Self {
        Self(parts)
    }
}

/// Tool approval response for provider-executed tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalResponse {
    /// The approval ID.
    pub approval_id: String,
    /// Whether the tool call was approved.
    pub approved: bool,
    /// Optional reason for the approval or denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the tool call is provider-executed.
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
