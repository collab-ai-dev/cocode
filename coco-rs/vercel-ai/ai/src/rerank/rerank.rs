//! Rerank documents using a reranking model.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::RerankingModelV4;
use vercel_ai_provider::reranking_model::RerankDocuments;
use vercel_ai_provider::reranking_model::RerankingModelV4CallOptions;

use crate::error::AIError;
use crate::logger::LogWarningsOptions;
use crate::logger::log_warnings;
use crate::provider::get_default_provider;
use crate::rerank::rerank_result::RerankResponse;
use crate::rerank::rerank_result::RerankResult;
use crate::rerank::rerank_result::RerankedDocument;
use crate::telemetry::TelemetrySettings;
use crate::types::ProviderOptions;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

/// A reference to a reranking model.
#[derive(Clone)]
pub enum RerankingModel {
    /// A string model ID that will be resolved via the default provider.
    String(String),
    /// A pre-resolved reranking model.
    V4(Arc<dyn RerankingModelV4>),
}

impl Default for RerankingModel {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl RerankingModel {
    /// Create from a string ID.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self::String(id.into())
    }

    /// Create from a V4 model.
    pub fn from_v4(model: Arc<dyn RerankingModelV4>) -> Self {
        Self::V4(model)
    }
}

impl From<String> for RerankingModel {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for RerankingModel {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl From<Arc<dyn RerankingModelV4>> for RerankingModel {
    fn from(model: Arc<dyn RerankingModelV4>) -> Self {
        Self::V4(model)
    }
}

/// Options for `rerank`.
#[derive(Default)]
pub struct RerankOptions {
    /// The reranking model to use.
    pub model: RerankingModel,
    /// The documents that should be reranked.
    pub documents: Vec<String>,
    /// The query to rerank the documents against.
    pub query: String,
    /// Number of top documents to return.
    pub top_n: Option<usize>,
    /// Maximum number of retries. Set to 0 to disable retries.
    pub max_retries: Option<u32>,
    /// Abort signal.
    pub abort_signal: Option<CancellationToken>,
    /// Additional headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Optional telemetry configuration (experimental).
    pub telemetry: Option<TelemetrySettings>,
    /// Additional provider-specific options.
    pub provider_options: Option<ProviderOptions>,
}

impl RerankOptions {
    /// Create new options with a model, query, and documents.
    pub fn new(
        model: impl Into<RerankingModel>,
        query: impl Into<String>,
        documents: Vec<String>,
    ) -> Self {
        Self {
            model: model.into(),
            query: query.into(),
            documents,
            ..Default::default()
        }
    }

    /// Set the number of top documents to return.
    pub fn with_top_n(mut self, top_n: usize) -> Self {
        self.top_n = Some(top_n);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
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

    /// Set telemetry configuration.
    pub fn with_telemetry(mut self, telemetry: TelemetrySettings) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

/// Resolve a reranking model reference to an actual model instance.
fn resolve_reranking_model(model: RerankingModel) -> Result<Arc<dyn RerankingModelV4>, AIError> {
    match model {
        RerankingModel::V4(m) => Ok(m),
        RerankingModel::String(id) => {
            let provider = get_default_provider().ok_or_else(|| {
                AIError::InvalidArgument(
                    "No default provider set. Call set_default_provider() first or use a RerankingModel::V4 variant.".to_string(),
                )
            })?;
            provider
                .as_ref()
                .reranking_model(&id)
                .map_err(|e| AIError::ProviderError(AISdkError::new(e.to_string())))
        }
    }
}

/// Rerank documents using a reranking model.
///
/// The type of the value is defined by the reranking model.
///
/// # Arguments
///
/// * `options` - The options including model, query, and documents.
///
/// # Returns
///
/// A `RerankResult` containing the reranked documents with scores.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{rerank, RerankOptions};
///
/// let result = rerank(RerankOptions {
///     model: "rerank-1".into(),
///     query: "What is machine learning?".to_string(),
///     documents: vec![
///         "Machine learning is a subset of AI.".to_string(),
///         "Python is a programming language.".to_string(),
///     ],
///     ..Default::default()
/// }).await?;
///
/// for doc in result.ranking {
///     println!("Score {}: {:?}", doc.score, doc.document);
/// }
/// ```
pub async fn rerank(options: RerankOptions) -> Result<RerankResult<String>, AIError> {
    // Handle empty documents case
    if options.documents.is_empty() {
        let model = resolve_reranking_model(options.model)?;
        return Ok(RerankResult::new(
            vec![],
            vec![],
            RerankResponse::new(model.model_id()),
        ));
    }

    let model = resolve_reranking_model(options.model)?;
    let model_id = model.model_id().to_string();
    let provider = model.provider().to_string();

    let mut call_options = RerankingModelV4CallOptions::new(&options.query, options.documents);

    if let Some(top_n) = options.top_n {
        call_options.top_n = Some(top_n);
    }
    if let Some(signal) = options.abort_signal {
        call_options.abort_signal = Some(signal);
    }
    if let Some(headers) = options.headers {
        call_options.headers = Some(headers);
    }
    if let Some(provider_opts) = options.provider_options {
        call_options.provider_options = Some(provider_opts);
    }

    // Build retry config
    let retry_config = options
        .max_retries
        .map(|max_retries| RetryConfig::new().with_max_retries(max_retries))
        .unwrap_or_default();

    // Store original documents for result construction
    let original_documents = match &call_options.documents {
        RerankDocuments::Text(docs) => docs.clone(),
        RerankDocuments::Object(_) => Vec::new(),
    };

    // Execute with retry
    let model_clone = model.clone();
    let result = with_retry(retry_config, None, || {
        let model = model_clone.clone();
        let call_options = call_options.clone();
        async move { model.do_rerank(call_options).await.map_err(AIError::from) }
    })
    .await?;

    // Log warnings from the provider result
    if let Some(ref warnings) = result.warnings {
        log_warnings(&LogWarningsOptions::new(
            warnings.clone(),
            &provider,
            &model_id,
        ));
    }

    // Build result
    let ranking: Vec<RerankedDocument<String>> = result
        .results
        .into_iter()
        .map(|r| {
            let doc = original_documents.get(r.index).cloned().unwrap_or_default();
            RerankedDocument::new(r.index, r.relevance_score, doc)
        })
        .collect();

    let mut response = RerankResponse::new(&model_id);
    if let Some(ref resp) = result.response {
        if let Some(ref id) = resp.id {
            response = response.with_id(id);
        }
        if let Some(ts) = resp.timestamp {
            response = response.with_timestamp(ts);
        }
        if let Some(ref headers) = resp.headers {
            response = response.with_headers(headers.clone());
        }
        if let Some(ref body) = resp.body {
            response = response.with_body(body.clone());
        }
    }

    let mut rerank_result = RerankResult::new(original_documents, ranking, response);
    if let Some(metadata) = result.provider_metadata {
        rerank_result = rerank_result.with_provider_metadata(metadata);
    }

    Ok(rerank_result)
}

#[cfg(test)]
#[path = "rerank.test.rs"]
mod tests;
