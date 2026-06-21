//! Rerank result types.

use vercel_ai_provider::ProviderMetadata;

/// Result of a `rerank` call.
#[derive(Debug)]
#[must_use]
pub struct RerankResult<T = String> {
    /// The original documents that were reranked.
    pub original_documents: Vec<T>,
    /// The ranking is a list of objects with the original index,
    /// relevance score, and the reranked document.
    ///
    /// Sorted by relevance score in descending order.
    /// Can be less than the original documents if there was a top_n limit.
    pub ranking: Vec<RerankedDocument<T>>,
    /// Optional provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Response information.
    pub response: RerankResponse,
}

impl<T> RerankResult<T> {
    /// Create a new rerank result.
    pub fn new(
        original_documents: Vec<T>,
        ranking: Vec<RerankedDocument<T>>,
        response: RerankResponse,
    ) -> Self {
        Self {
            original_documents,
            ranking,
            provider_metadata: None,
            response,
        }
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Get the reranked documents in order.
    pub fn reranked_documents(&self) -> Vec<&T> {
        self.ranking.iter().map(|r| &r.document).collect()
    }
}

/// A reranked document with relevance score.
#[derive(Debug, Clone)]
pub struct RerankedDocument<T = String> {
    /// The index of the document in the original list.
    pub original_index: usize,
    /// The relevance score (0-1).
    pub score: f64,
    /// The document.
    pub document: T,
}

impl<T> RerankedDocument<T> {
    /// Create a new reranked document.
    pub fn new(original_index: usize, score: f64, document: T) -> Self {
        Self {
            original_index,
            score,
            document,
        }
    }
}

/// Response information from a rerank call.
#[derive(Debug, Clone)]
pub struct RerankResponse {
    /// ID for the generated response if the provider sends one.
    pub id: Option<String>,
    /// Timestamp of the generated response.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// The ID of the model that was used to generate the response.
    pub model_id: String,
    /// Response headers.
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl RerankResponse {
    /// Create a new rerank response.
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            id: None,
            timestamp: chrono::Utc::now(),
            model_id: model_id.into(),
            headers: None,
            body: None,
        }
    }

    /// Set the response ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Set the headers.
    pub fn with_headers(mut self, headers: std::collections::HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}
