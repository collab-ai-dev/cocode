//! Response metadata types.
//!
//! This module provides types for capturing request and response metadata
//! from language model API calls.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

/// Metadata about the request sent to the language model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanguageModelRequestMetadata {
    /// The raw request body, if available.
    pub body: Option<serde_json::Value>,
}

impl LanguageModelRequestMetadata {
    /// Create new request metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create request metadata with a body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Metadata about the response from the language model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanguageModelResponseMetadata {
    /// The response ID from the provider.
    pub id: Option<String>,
    /// The timestamp of the response.
    /// The timestamp of the response (spec: `LanguageModelV4ResponseMetadata.timestamp`).
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used for the request.
    pub model_id: Option<String>,
    /// Response headers from the provider.
    pub headers: Option<HashMap<String, String>>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl LanguageModelResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create response metadata with an ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Create response metadata with a timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Create response metadata with a model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Create response metadata with headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Create response metadata with a body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Response metadata for video generation.
#[derive(Debug, Clone, Default)]
pub struct VideoModelResponseMetadata {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

impl VideoModelResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }
}

/// Response metadata for speech generation.
#[derive(Debug, Clone, Default)]
pub struct SpeechModelResponseMetadata {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl SpeechModelResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set the response body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Response metadata for image generation.
#[derive(Debug, Clone, Default)]
pub struct ImageModelResponseMetadata {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

impl ImageModelResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set response headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }
}

/// Response metadata for transcription.
#[derive(Debug, Clone, Default)]
pub struct TranscriptionModelResponseMetadata {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl TranscriptionModelResponseMetadata {
    /// Create new response metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set the response body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

#[cfg(test)]
#[path = "response_metadata.test.rs"]
mod tests;
