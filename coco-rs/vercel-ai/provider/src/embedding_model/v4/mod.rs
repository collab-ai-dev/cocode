//! Embedding model trait and related types (V4).
//!
//! This module defines the `EmbeddingModelV4` trait for implementing embedding models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

/// The embedding model trait (V4).
///
/// This trait defines the interface for embedding models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait EmbeddingModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Get the maximum number of embeddings per call.
    fn max_embeddings_per_call(&self) -> usize {
        1
    }

    /// Whether the model supports parallel calls.
    /// When true, multiple embedding requests can be sent in parallel.
    fn supports_parallel_calls(&self) -> bool {
        false
    }

    /// Get the supported embedding dimensions.
    ///
    /// Note: This is a coco-rs extension not present in the upstream spec.
    fn supported_dimensions(&self) -> Option<Vec<usize>> {
        None
    }

    /// Generate embeddings.
    async fn do_embed(
        &self,
        options: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4EmbedResult, AISdkError>;
}

/// Options for an embedding model call.
#[derive(Debug, Clone, Default)]
pub struct EmbeddingModelV4CallOptions {
    /// The values to embed.
    pub values: Vec<String>,
    /// The embedding dimensions (if supported).
    pub dimensions: Option<usize>,
    /// The embedding type.
    pub embedding_type: Option<EmbeddingType>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl EmbeddingModelV4CallOptions {
    /// Create new call options with values.
    pub fn new(values: Vec<String>) -> Self {
        Self {
            values,
            ..Default::default()
        }
    }

    /// Set the embedding dimensions.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Set the embedding type.
    pub fn with_embedding_type(mut self, embedding_type: EmbeddingType) -> Self {
        self.embedding_type = Some(embedding_type);
        self
    }

    /// Set provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }
}

/// The type of embedding to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingType {
    /// Dense embeddings (default).
    #[default]
    Dense,
    /// Sparse embeddings.
    Sparse,
}

/// The result of an embedding call.
#[derive(Debug, Clone)]
pub struct EmbeddingModelV4EmbedResult {
    /// The generated embeddings.
    pub embeddings: Vec<EmbeddingValue>,
    /// Token usage.
    pub usage: EmbeddingUsage,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// The raw response (for debugging).
    pub raw_response: Option<serde_json::Value>,
}

impl EmbeddingModelV4EmbedResult {
    /// Create a new embedding result.
    pub fn new(embeddings: Vec<EmbeddingValue>, usage: EmbeddingUsage) -> Self {
        Self {
            embeddings,
            usage,
            warnings: Vec::new(),
            provider_metadata: None,
            raw_response: None,
        }
    }

    /// Create from float vectors.
    pub fn from_vectors(vectors: Vec<Vec<f32>>, usage: EmbeddingUsage) -> Self {
        let embeddings = vectors.into_iter().map(EmbeddingValue::dense).collect();
        Self::new(embeddings, usage)
    }

    /// Get the dense embeddings as vectors.
    pub fn dense_vectors(&self) -> Vec<&Vec<f32>> {
        self.embeddings
            .iter()
            .filter_map(|e| match e {
                EmbeddingValue::Dense { vector } => Some(vector),
                _ => None,
            })
            .collect()
    }
}

/// An embedding value.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddingValue {
    /// Dense embedding.
    Dense {
        /// The embedding vector.
        vector: Vec<f32>,
    },
    /// Sparse embedding.
    Sparse {
        /// The sparse embedding indices.
        indices: Vec<usize>,
        /// The sparse embedding values.
        values: Vec<f32>,
    },
}

impl EmbeddingValue {
    /// Create a dense embedding.
    pub fn dense(vector: Vec<f32>) -> Self {
        Self::Dense { vector }
    }

    /// Create a sparse embedding.
    pub fn sparse(indices: Vec<usize>, values: Vec<f32>) -> Self {
        Self::Sparse { indices, values }
    }

    /// Get the dense vector if this is a dense embedding.
    pub fn as_dense(&self) -> Option<&Vec<f32>> {
        match self {
            Self::Dense { vector } => Some(vector),
            _ => None,
        }
    }
}

/// Token usage for embedding calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingUsage {
    /// The number of tokens in the input.
    pub prompt_tokens: u64,
    /// The total number of tokens.
    pub total_tokens: u64,
}

impl EmbeddingUsage {
    /// Create new embedding usage.
    pub fn new(prompt_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            total_tokens: prompt_tokens,
        }
    }
}

#[cfg(test)]
#[path = "embedding_model_v4.test.rs"]
mod tests;
