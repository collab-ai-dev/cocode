//! Language model V4 response metadata type.
//!
//! Metadata about a generated response.

use serde::Deserialize;
use serde::Serialize;

/// Metadata about a generated response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageModelV4ResponseMetadata {
    /// ID for the generated response, if the provider sends one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Timestamp for the start of the generated response, if the provider sends one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// The ID of the response model that was used to generate the response, if the provider sends one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

impl LanguageModelV4ResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self {
            id: None,
            timestamp: None,
            model_id: None,
        }
    }

    /// Set the response ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }
}

impl Default for LanguageModelV4ResponseMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "response_metadata.test.rs"]
mod tests;
