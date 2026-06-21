//! Language model V4 content type.
//!
//! Union type for all content types that can appear in a response.

use serde::Deserialize;
use serde::Serialize;

use super::file::LanguageModelV4File;
use super::file::LanguageModelV4FileData;
use super::reasoning::LanguageModelV4Reasoning;
use super::source::LanguageModelV4Source;
use super::text::LanguageModelV4Text;
use super::tool_approval_request::LanguageModelV4ToolApprovalRequest;
use super::tool_call::LanguageModelV4ToolCall;
use super::tool_result::LanguageModelV4ToolResult;
use crate::shared::ProviderMetadata;

/// Content that can appear in a model response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LanguageModelV4Content {
    /// Text content.
    Text(LanguageModelV4Text),
    /// Reasoning content (for thinking models).
    Reasoning(LanguageModelV4Reasoning),
    /// File content (generated files).
    File(LanguageModelV4File),
    /// Tool approval request (for provider-executed tools).
    ToolApprovalRequest(LanguageModelV4ToolApprovalRequest),
    /// Source reference (for citations).
    Source(LanguageModelV4Source),
    /// Reasoning file content (file data that is part of reasoning).
    #[serde(rename = "reasoning-file")]
    ReasoningFile {
        /// File data as a tagged 2-arm union (data or url).
        data: LanguageModelV4FileData,
        /// Either a full IANA media type or just the top-level segment.
        #[serde(rename = "mediaType")]
        media_type: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Custom content for provider-specific extensions.
    Custom {
        /// The kind of custom content.
        kind: String,
        /// Provider metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Tool call.
    ToolCall(LanguageModelV4ToolCall),
    /// Tool result.
    ToolResult(LanguageModelV4ToolResult),
}

impl LanguageModelV4Content {
    /// Create text content.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(LanguageModelV4Text::new(text))
    }

    /// Create reasoning content.
    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::Reasoning(LanguageModelV4Reasoning::new(text))
    }

    /// Check if this is text content.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// Check if this is reasoning content.
    pub fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning(_))
    }

    /// Check if this is a tool call.
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall(_))
    }

    /// Check if this is a tool result.
    pub fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult(_))
    }

    /// Get the text content if this is text.
    pub fn as_text(&self) -> Option<&LanguageModelV4Text> {
        match self {
            Self::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Get the tool call if this is a tool call.
    pub fn as_tool_call(&self) -> Option<&LanguageModelV4ToolCall> {
        match self {
            Self::ToolCall(c) => Some(c),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "content.test.rs"]
mod tests;
