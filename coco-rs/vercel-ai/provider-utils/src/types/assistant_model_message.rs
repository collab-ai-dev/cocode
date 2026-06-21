//! Assistant model message type.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::ProviderOptions;

/// An assistant message.
///
/// It can contain text, tool calls, or a combination of text and tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantModelMessage {
    /// The role, always "assistant".
    pub role: String,
    /// The message content.
    pub content: AssistantContent,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl AssistantModelMessage {
    /// Create a new assistant message with text.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: AssistantContent::Text(content.into()),
            provider_options: None,
        }
    }

    /// Create a new assistant message with content parts.
    pub fn parts(parts: Vec<AssistantContentPart>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: AssistantContent::Parts(parts),
            provider_options: None,
        }
    }

    /// Add provider options.
    pub fn with_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

/// Content of an assistant message.
///
/// It can be a string or an array of text, file, reasoning, and tool call parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssistantContent {
    /// Text content.
    Text(String),
    /// Multiple content parts.
    Parts(Vec<AssistantContentPart>),
}

impl From<String> for AssistantContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for AssistantContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}
