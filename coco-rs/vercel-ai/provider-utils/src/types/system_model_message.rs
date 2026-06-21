//! System model message type.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ProviderOptions;

/// A system message.
///
/// It can contain system information.
///
/// Note: using the "system" part of the prompt is strongly preferred
/// to increase the resilience against prompt injection attacks,
/// and because not all providers support several system messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemModelMessage {
    /// The role, always "system".
    pub role: String,
    /// The message content.
    pub content: String,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl SystemModelMessage {
    /// Create a new system message.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            provider_options: None,
        }
    }

    /// Add provider options.
    pub fn with_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

impl From<String> for SystemModelMessage {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for SystemModelMessage {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}
