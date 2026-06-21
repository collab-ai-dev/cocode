//! Language model V4 tool result type.
//!
//! Result of a tool call that has been executed by the provider.

use crate::json_value::JSONValue;
use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// Result of a tool call that has been executed by the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageModelV4ToolResult {
    /// The ID of the tool call that this result is associated with.
    pub tool_call_id: String,
    /// Name of the tool that generated this result.
    pub tool_name: String,
    /// Result of the tool call. This is a JSON-serializable object.
    pub result: JSONValue,
    /// Optional flag if the result is an error or an error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Whether the tool result is preliminary.
    ///
    /// Preliminary tool results replace each other, e.g. image previews.
    /// There always has to be a final, non-preliminary tool result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preliminary: Option<bool>,
    /// Whether the tool is dynamic, i.e. defined at runtime.
    /// For example, MCP (Model Context Protocol) tools that are executed by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl LanguageModelV4ToolResult {
    /// Create a new tool result.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        result: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result,
            is_error: None,
            preliminary: None,
            dynamic: None,
            provider_metadata: None,
        }
    }

    /// Create an error tool result.
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result: error,
            is_error: Some(true),
            preliminary: None,
            dynamic: None,
            provider_metadata: None,
        }
    }

    /// Set whether this is an error result.
    pub fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = Some(is_error);
        self
    }

    /// Set whether this is a preliminary result.
    pub fn with_preliminary(mut self, preliminary: bool) -> Self {
        self.preliminary = Some(preliminary);
        self
    }

    /// Set whether this is from a dynamic tool.
    pub fn with_dynamic(mut self, dynamic: bool) -> Self {
        self.dynamic = Some(dynamic);
        self
    }

    /// Set provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

#[cfg(test)]
#[path = "tool_result.test.rs"]
mod tests;
