//! Language model V4 text content type.
//!
//! Text content that the model has generated.

use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// Text that the model has generated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageModelV4Text {
    /// The text content.
    pub text: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl LanguageModelV4Text {
    /// Create a new text content.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provider_metadata: None,
        }
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

impl From<String> for LanguageModelV4Text {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for LanguageModelV4Text {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

#[cfg(test)]
#[path = "text.test.rs"]
mod tests;
