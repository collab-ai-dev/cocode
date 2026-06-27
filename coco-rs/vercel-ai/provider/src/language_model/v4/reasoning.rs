//! Language model V4 reasoning content type.
//!
//! Reasoning content that the model has generated (for thinking models).

use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// Reasoning that the model has generated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageModelV4Reasoning {
    /// The reasoning text.
    pub text: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl LanguageModelV4Reasoning {
    /// Create a new reasoning content.
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

impl From<String> for LanguageModelV4Reasoning {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for LanguageModelV4Reasoning {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

#[cfg(test)]
#[path = "reasoning.test.rs"]
mod tests;
