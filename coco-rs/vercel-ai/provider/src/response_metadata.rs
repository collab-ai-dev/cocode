//! Response metadata types.

use serde::Deserialize;
use serde::Serialize;

use crate::shared::ProviderMetadata;

/// Metadata about a model response.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// The response ID from the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The model used for the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The timestamp of the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the response ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// Information about a model response.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseInfo {
    /// The response ID.
    pub id: String,
    /// The timestamp of the response.
    pub timestamp: Option<String>,
    /// The model ID used.
    pub model_id: Option<String>,
}

impl ResponseInfo {
    /// Create new response info.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            timestamp: None,
            model_id: None,
        }
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
