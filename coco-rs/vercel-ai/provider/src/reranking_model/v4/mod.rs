//! Reranking model trait and related types (V4).
//!
//! This module defines the `RerankingModelV4` trait for implementing reranking models
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

/// The reranking model trait (V4).
///
/// This trait defines the interface for reranking models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait RerankingModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Rerank documents based on a query.
    async fn do_rerank(
        &self,
        options: RerankingModelV4CallOptions,
    ) -> Result<RerankingModelV4Result, AISdkError>;
}

/// Documents to rerank, either as plain text or JSON objects.
#[derive(Debug, Clone)]
pub enum RerankDocuments {
    /// Plain text documents.
    Text(Vec<String>),
    /// JSON object documents.
    Object(Vec<serde_json::Value>),
}

impl Default for RerankDocuments {
    fn default() -> Self {
        Self::Text(Vec::new())
    }
}

impl RerankDocuments {
    /// Get the number of documents.
    pub fn len(&self) -> usize {
        match self {
            Self::Text(v) => v.len(),
            Self::Object(v) => v.len(),
        }
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl From<Vec<String>> for RerankDocuments {
    fn from(docs: Vec<String>) -> Self {
        Self::Text(docs)
    }
}

impl From<Vec<serde_json::Value>> for RerankDocuments {
    fn from(docs: Vec<serde_json::Value>) -> Self {
        Self::Object(docs)
    }
}

/// Options for a reranking model call.
#[derive(Debug, Clone, Default)]
pub struct RerankingModelV4CallOptions {
    /// The query to rank documents against.
    pub query: String,
    /// The documents to rank.
    pub documents: RerankDocuments,
    /// The number of top results to return.
    pub top_n: Option<usize>,
    /// Whether to return documents in the result.
    pub return_documents: Option<bool>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl RerankingModelV4CallOptions {
    /// Create new call options with text documents.
    pub fn new(query: impl Into<String>, documents: Vec<String>) -> Self {
        Self {
            query: query.into(),
            documents: RerankDocuments::Text(documents),
            ..Default::default()
        }
    }

    /// Create new call options with a documents enum.
    pub fn with_documents(query: impl Into<String>, documents: RerankDocuments) -> Self {
        Self {
            query: query.into(),
            documents,
            ..Default::default()
        }
    }

    /// Set the number of top results.
    pub fn with_top_n(mut self, top_n: usize) -> Self {
        self.top_n = Some(top_n);
        self
    }

    /// Set whether to return documents.
    pub fn with_return_documents(mut self, return_documents: bool) -> Self {
        self.return_documents = Some(return_documents);
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
}

/// The result of a reranking call.
#[derive(Debug, Clone)]
pub struct RerankingModelV4Result {
    /// The reranked items (index + score only at provider level).
    pub results: Vec<RankedItem>,
    /// Token usage (if available).
    pub usage: Option<RerankingUsage>,
    /// Warnings from the provider.
    pub warnings: Option<Vec<Warning>>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Response metadata.
    pub response: Option<RerankingModelV4Response>,
}

impl RerankingModelV4Result {
    /// Create a new reranking result.
    pub fn new(results: Vec<RankedItem>) -> Self {
        Self {
            results,
            usage: None,
            warnings: None,
            provider_metadata: None,
            response: None,
        }
    }

    /// Set the usage.
    pub fn with_usage(mut self, usage: RerankingUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Set warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = Some(warnings);
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set response metadata.
    pub fn with_response(mut self, response: RerankingModelV4Response) -> Self {
        self.response = Some(response);
        self
    }
}

/// A ranked item from the provider (index + relevance score only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedItem {
    /// The index of the document in the original list.
    pub index: usize,
    /// The relevance score.
    pub relevance_score: f64,
}

impl RankedItem {
    /// Create a new ranked item.
    pub fn new(index: usize, relevance_score: f64) -> Self {
        Self {
            index,
            relevance_score,
        }
    }
}

/// Response metadata from a reranking call.
#[derive(Debug, Clone, Default)]
pub struct RerankingModelV4Response {
    /// Response ID.
    pub id: Option<String>,
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl RerankingModelV4Response {
    /// Set the ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
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
        self.headers = Some(headers);
        self
    }

    /// Set the response body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Token usage for reranking calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerankingUsage {
    /// The number of tokens in the prompt.
    pub prompt_tokens: u64,
    /// The total number of tokens.
    pub total_tokens: u64,
}

impl RerankingUsage {
    /// Create new reranking usage.
    pub fn new(prompt_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            total_tokens: prompt_tokens,
        }
    }
}

#[cfg(test)]
#[path = "reranking_model_v4.test.rs"]
mod tests;
