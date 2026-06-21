//! Stream part types for streaming responses.
//!
//! These types represent the granular events emitted during streaming.

use serde::Deserialize;
use serde::Serialize;

use super::finish_reason::FinishReason;
use super::tool_approval_request::LanguageModelV4ToolApprovalRequest;
use super::tool_call::LanguageModelV4ToolCall;
use super::tool_result::LanguageModelV4ToolResult;
use super::usage::Usage;
use crate::json_value::JSONValue;
use crate::response_metadata::ResponseMetadata;
use crate::shared::ProviderMetadata;
use crate::shared::Warning;

/// Backward-compatible alias for [`LanguageModelV4ToolApprovalRequest`].
pub type ToolApprovalRequest = LanguageModelV4ToolApprovalRequest;

/// Backward-compatible alias for [`crate::content::SourcePart`].
pub type Source = crate::content::SourcePart;

/// Backward-compatible alias for [`crate::content::SourceType`].
pub type SourceType = crate::content::SourceType;

/// A file in a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    /// The file data (base64 or URL).
    pub data: String,
    /// The MIME type.
    pub media_type: String,
    /// Provider metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// A reasoning file in a response (file data that is part of reasoning).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningFile {
    /// The file data (base64).
    pub data: String,
    /// The MIME type.
    pub media_type: String,
    /// Provider metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// A stream part emitted during streaming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LanguageModelV4StreamPart {
    /// Emitted when a new text segment starts.
    TextStart {
        /// The text segment ID.
        id: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted for each text delta.
    TextDelta {
        /// The text segment ID.
        id: String,
        /// The text delta.
        delta: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when a text segment ends.
    TextEnd {
        /// The text segment ID.
        id: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when reasoning starts.
    ReasoningStart {
        /// The reasoning segment ID.
        id: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted for each reasoning delta.
    ReasoningDelta {
        /// The reasoning segment ID.
        id: String,
        /// The reasoning delta.
        delta: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when reasoning ends.
    ReasoningEnd {
        /// The reasoning segment ID.
        id: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when a tool call input starts.
    ToolInputStart {
        /// The tool call ID.
        id: String,
        /// The tool name.
        tool_name: String,
        /// Whether the tool is executed by the provider.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        /// Whether the tool is dynamic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dynamic: Option<bool>,
        /// The title of the tool call (for display).
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted for each tool input delta.
    ToolInputDelta {
        /// The tool call ID.
        id: String,
        /// The input delta.
        delta: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when tool input ends.
    ToolInputEnd {
        /// The tool call ID.
        id: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when a tool call is complete.
    #[serde(rename = "tool-call")]
    ToolCall(LanguageModelV4ToolCall),
    /// Emitted when a tool result is available.
    #[serde(rename = "tool-result")]
    ToolResult(LanguageModelV4ToolResult),
    /// Emitted when a provider requests approval for a tool execution.
    #[serde(rename = "tool-approval-request")]
    ToolApprovalRequest(LanguageModelV4ToolApprovalRequest),
    /// Emitted when a file is available.
    #[serde(rename = "file")]
    File(File),
    /// Emitted when a reasoning file is available (file data that is part of reasoning).
    #[serde(rename = "reasoning-file")]
    ReasoningFile(ReasoningFile),
    /// Emitted for custom provider-specific content.
    #[serde(rename = "custom")]
    Custom {
        /// The kind of custom content.
        kind: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted when a source is available.
    #[serde(rename = "source")]
    Source(Source),
    /// Emitted at the start of the stream.
    StreamStart {
        /// Warnings from the provider.
        warnings: Vec<Warning>,
    },
    /// Emitted with response metadata.
    #[serde(rename = "response-metadata")]
    ResponseMetadata(ResponseMetadata),
    /// Emitted when the response is finished.
    Finish {
        /// Token usage.
        usage: Usage,
        /// The finish reason.
        finish_reason: FinishReason,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Emitted for raw data (for debugging/intermediate processing).
    Raw {
        /// The raw value.
        raw_value: JSONValue,
    },
    /// Emitted when an error occurs.
    Error {
        /// The error.
        error: StreamError,
    },
}

/// An error that occurred during streaming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamError {
    /// The error message.
    pub message: String,
    /// The error code (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Whether the error is retryable.
    #[serde(default)]
    pub is_retryable: bool,
}

impl StreamError {
    /// Create a new stream error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            is_retryable: false,
        }
    }

    /// Create a retryable error.
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            is_retryable: true,
        }
    }
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod tests;
