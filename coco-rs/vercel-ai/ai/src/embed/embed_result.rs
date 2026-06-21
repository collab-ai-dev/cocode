//! Embedding result types.

use std::collections::HashMap;

use vercel_ai_provider::EmbeddingUsage;
use vercel_ai_provider::EmbeddingValue;
use vercel_ai_provider::Warning;

/// Provider-specific metadata.
pub type ProviderMetadata = HashMap<String, serde_json::Map<String, serde_json::Value>>;

/// Response data from a model call.
#[derive(Debug, Clone)]
pub struct ResponseData {
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
    /// The response body.
    pub body: Option<serde_json::Value>,
}

impl ResponseData {
    /// Create new response data.
    pub fn new() -> Self {
        Self {
            headers: None,
            body: None,
        }
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

impl Default for ResponseData {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of an `embed` call.
#[derive(Debug)]
#[must_use]
pub struct EmbedResult {
    /// The value that was embedded.
    pub value: String,
    /// The embedding vector.
    pub embedding: EmbeddingValue,
    /// Token usage.
    pub usage: EmbeddingUsage,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Optional provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Optional response data.
    pub response: Option<ResponseData>,
    /// Raw response from the provider (for debugging).
    pub raw_response: Option<serde_json::Value>,
}

impl EmbedResult {
    /// Create a new embed result.
    pub fn new(value: impl Into<String>, embedding: EmbeddingValue, usage: EmbeddingUsage) -> Self {
        Self {
            value: value.into(),
            embedding,
            usage,
            warnings: Vec::new(),
            provider_metadata: None,
            response: None,
            raw_response: None,
        }
    }

    /// Set warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set response data.
    pub fn with_response(mut self, response: ResponseData) -> Self {
        self.response = Some(response);
        self
    }

    /// Set raw response.
    pub fn with_raw_response(mut self, raw_response: serde_json::Value) -> Self {
        self.raw_response = Some(raw_response);
        self
    }

    /// Get the dense embedding vector if available.
    pub fn as_dense(&self) -> Option<&Vec<f32>> {
        self.embedding.as_dense()
    }
}

/// Result of an `embed_many` call.
#[derive(Debug)]
#[must_use]
pub struct EmbedManyResult {
    /// The values that were embedded.
    pub values: Vec<String>,
    /// The embedding vectors.
    pub embeddings: Vec<EmbeddingValue>,
    /// Token usage.
    pub usage: EmbeddingUsage,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Optional provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Optional response data for each chunk.
    pub responses: Vec<Option<ResponseData>>,
    /// Raw responses from the provider (one per chunk call).
    pub raw_responses: Vec<serde_json::Value>,
}

impl EmbedManyResult {
    /// Create a new embed many result.
    pub fn new(
        values: Vec<String>,
        embeddings: Vec<EmbeddingValue>,
        usage: EmbeddingUsage,
    ) -> Self {
        Self {
            values,
            embeddings,
            usage,
            warnings: Vec::new(),
            provider_metadata: None,
            responses: Vec::new(),
            raw_responses: Vec::new(),
        }
    }

    /// Set warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set responses.
    pub fn with_responses(mut self, responses: Vec<Option<ResponseData>>) -> Self {
        self.responses = responses;
        self
    }

    /// Set raw responses.
    pub fn with_raw_responses(mut self, raw_responses: Vec<serde_json::Value>) -> Self {
        self.raw_responses = raw_responses;
        self
    }

    /// Get all dense embedding vectors.
    pub fn dense_vectors(&self) -> Vec<&Vec<f32>> {
        self.embeddings
            .iter()
            .filter_map(|e| e.as_dense())
            .collect()
    }
}
