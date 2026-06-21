//! User model message type.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::UserContentPart;

/// A user message.
///
/// It can contain text or a combination of text and images.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserModelMessage {
    /// The role, always "user".
    pub role: String,
    /// The message content.
    pub content: UserContent,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl UserModelMessage {
    /// Create a new user message with text.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: UserContent::Text(content.into()),
            provider_options: None,
        }
    }

    /// Create a new user message with content parts.
    pub fn parts(parts: Vec<UserContentPart>) -> Self {
        Self {
            role: "user".to_string(),
            content: UserContent::Parts(parts),
            provider_options: None,
        }
    }

    /// Add provider options.
    pub fn with_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

/// Content of a user message.
///
/// It can be a string or an array of text, image, and file parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    /// Text content.
    Text(String),
    /// Multiple content parts.
    Parts(Vec<UserContentPart>),
}

impl From<String> for UserContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for UserContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}
