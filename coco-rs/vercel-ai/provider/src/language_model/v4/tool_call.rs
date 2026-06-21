//! Language model V4 tool call type.
//!
//! Tool calls that the model has generated.

use crate::content::ToolInputInvalidReason;
use crate::json_value::JSONValue;
use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// Tool calls that the model has generated.
///
/// **Coco-rs-specific extension**: `invalid` + `invalid_reason` mirror the
/// fields on the post-stream [`crate::ToolCallPart`] so a provider adapter
/// that detects an unrecoverable wire-parsing parse failure (e.g. Anthropic
/// streaming `content_block_stop` flush) can carry the structured
/// classification through the stream to the consumer. Without this the
/// stream type would silently drop the reason and the agent loop would
/// fall back to its generic error message.
///
/// TS upstream (`@ai-sdk/provider` v4) does not carry these fields; the
/// deviation is documented alongside `UnifiedFinishReason`'s extra variants
/// in this crate's CLAUDE.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageModelV4ToolCall {
    /// The identifier of the tool call. It must be unique across all tool calls.
    pub tool_call_id: String,
    /// The name of the tool that should be called.
    pub tool_name: String,
    /// Stringified JSON object with the tool call arguments.
    /// Must match the parameters schema of the tool.
    pub input: String,
    /// Whether the tool call will be executed by the provider.
    /// If this flag is not set or is false, the tool call will be executed by the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    /// Whether the tool is dynamic, i.e. defined at runtime.
    /// For example, MCP (Model Context Protocol) tools that are executed by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
    /// `true` when the provider adapter detected an unrecoverable
    /// wire-parsing parse failure on this call. Engine consumers copy this
    /// flag onto the rebuilt [`crate::ToolCallPart`]. Default `false`
    /// plus `skip_serializing_if` keeps the wire body minimal on the
    /// happy path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invalid: bool,
    /// Structured reason accompanying [`Self::invalid`]. `Some(...)`
    /// only when `invalid == true`; the agent loop reads this to
    /// pick the `<tool_use_error>` wrap prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<ToolInputInvalidReason>,
}

impl LanguageModelV4ToolCall {
    /// Create a new tool call.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input: input.into(),
            provider_executed: None,
            dynamic: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        }
    }

    /// Coco-rs extension: mark the call as invalid with a structured
    /// reason. The agent loop reads the reason to pick the
    /// `<tool_use_error>` wrap prefix without string-matching.
    pub fn with_invalid_reason(mut self, reason: ToolInputInvalidReason) -> Self {
        self.invalid = true;
        self.invalid_reason = Some(reason);
        self
    }

    /// Create from JSON value (serializes to string).
    pub fn from_json(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: JSONValue,
    ) -> Self {
        Self::new(tool_call_id, tool_name, input.to_string())
    }

    /// Set whether this is a provider-executed tool call.
    pub fn with_provider_executed(mut self, provider_executed: bool) -> Self {
        self.provider_executed = Some(provider_executed);
        self
    }

    /// Set whether this is a dynamic tool call.
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
#[path = "tool_call.test.rs"]
mod tests;
