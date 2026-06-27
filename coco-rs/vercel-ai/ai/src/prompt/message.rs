//! Message types for prompts.
//!
//! This module defines the message types used in conversations.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ProviderOptions;

use super::content_part::PromptFilePart;
use super::content_part::PromptImagePart;
use super::content_part::PromptReasoningPart;
use super::content_part::PromptToolCallPart;
use super::content_part::PromptToolResultPart;

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum PromptMessage {
    /// System message.
    System(PromptSystemMessage),
    /// User message.
    User(PromptUserMessage),
    /// Assistant message.
    Assistant(PromptAssistantMessage),
    /// Tool message.
    Tool(PromptToolMessage),
}

/// System message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSystemMessage {
    /// The system message content.
    pub content: String,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl PromptSystemMessage {
    /// Create a new system message.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            provider_options: None,
        }
    }
}

/// User message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptUserMessage {
    /// The user message content.
    pub content: PromptUserContent,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// User message content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptUserContent {
    /// Simple text content.
    Text(String),
    /// Array of content parts.
    Parts(Vec<PromptUserContentPart>),
}

/// User content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptUserContentPart {
    /// Text part.
    Text {
        /// The text content.
        text: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Image part.
    Image(PromptImagePart),
    /// File part.
    File(PromptFilePart),
}

impl PromptUserMessage {
    /// Create a new user message with text.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: PromptUserContent::Text(content.into()),
            provider_options: None,
        }
    }

    /// Create a new user message with content parts.
    pub fn parts(parts: Vec<PromptUserContentPart>) -> Self {
        Self {
            content: PromptUserContent::Parts(parts),
            provider_options: None,
        }
    }
}

/// Assistant message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptAssistantMessage {
    /// The assistant message content.
    pub content: PromptAssistantContent,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// Assistant message content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptAssistantContent {
    /// Simple text content.
    Text(String),
    /// Array of content parts.
    Parts(Vec<PromptAssistantContentPart>),
}

/// Assistant content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptAssistantContentPart {
    /// Text part.
    Text {
        /// The text content.
        text: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// File part.
    File(PromptFilePart),
    /// Reasoning part.
    Reasoning(PromptReasoningPart),
    /// Tool call part.
    ToolCall(PromptToolCallPart),
    /// Tool result part.
    ToolResult(PromptToolResultPart),
}

impl PromptAssistantMessage {
    /// Create a new assistant message with text.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: PromptAssistantContent::Text(content.into()),
            provider_options: None,
        }
    }

    /// Create a new assistant message with content parts.
    pub fn parts(parts: Vec<PromptAssistantContentPart>) -> Self {
        Self {
            content: PromptAssistantContent::Parts(parts),
            provider_options: None,
        }
    }
}

/// Tool message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptToolMessage {
    /// The tool message content (array of tool results).
    pub content: Vec<PromptToolContentPart>,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// Tool content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptToolContentPart {
    /// Tool result part.
    ToolResult(PromptToolResultPart),
    /// Tool approval response.
    ToolApprovalResponse {
        /// The approval ID.
        approval_id: String,
        /// Whether approved.
        approved: bool,
        /// The reason (if denied).
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl PromptToolMessage {
    /// Create a new tool message.
    pub fn new(content: Vec<PromptToolContentPart>) -> Self {
        Self {
            content,
            provider_options: None,
        }
    }
}

#[cfg(test)]
#[path = "message.test.rs"]
mod tests;
