//! Generate embeddings from text.
//!
//! This module provides `embed` and `embed_many` functions for generating
//! embeddings from text using embedding models.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::embedding_model::EmbeddingModelV4;
use vercel_ai_provider::embedding_model::EmbeddingModelV4CallOptions;
use vercel_ai_provider::embedding_model::EmbeddingModelV4EmbedResult;
use vercel_ai_provider::embedding_model::EmbeddingType;
use vercel_ai_provider::embedding_model::EmbeddingUsage;
use vercel_ai_provider::embedding_model::EmbeddingValue;

use crate::error::AIError;
use crate::model::EmbeddingModel;
use crate::telemetry::TelemetrySettings;
use crate::types::ProviderOptions;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::embed_result::EmbedManyResult;
use super::embed_result::EmbedResult;

/// Options for `embed`.
#[derive(Default)]
pub struct EmbedOptions {
    /// The embedding model to use.
    pub model: EmbeddingModel,
    /// The text to embed.
    pub value: String,
    /// The embedding dimensions (if supported).
    pub dimensions: Option<usize>,
    /// The embedding type.
    pub embedding_type: Option<EmbeddingType>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Maximum number of retries for transient failures.
    pub max_retries: Option<u32>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Optional telemetry configuration (experimental).
    pub telemetry: Option<TelemetrySettings>,
}

impl EmbedOptions {
    /// Create new options with a model and value.
    pub fn new(model: impl Into<EmbeddingModel>, value: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            value: value.into(),
            ..Default::default()
        }
    }

    /// Set the dimensions.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Set the embedding type.
    pub fn with_embedding_type(mut self, embedding_type: EmbeddingType) -> Self {
        self.embedding_type = Some(embedding_type);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set telemetry configuration.
    pub fn with_telemetry(mut self, telemetry: TelemetrySettings) -> Self {
        self.telemetry = Some(telemetry);
        self
    }
}

/// Options for `embed_many`.
#[derive(Default)]
pub struct EmbedManyOptions {
    /// The embedding model to use.
    pub model: EmbeddingModel,
    /// The texts to embed.
    pub values: Vec<String>,
    /// The embedding dimensions (if supported).
    pub dimensions: Option<usize>,
    /// The embedding type.
    pub embedding_type: Option<EmbeddingType>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Maximum number of retries for transient failures.
    pub max_retries: Option<u32>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Maximum number of concurrent requests.
    /// Default is no limit (unbounded).
    pub max_parallel_calls: Option<usize>,
    /// Optional telemetry configuration (experimental).
    pub telemetry: Option<TelemetrySettings>,
}

impl EmbedManyOptions {
    /// Create new options with a model and values.
    pub fn new(model: impl Into<EmbeddingModel>, values: Vec<String>) -> Self {
        Self {
            model: model.into(),
            values,
            ..Default::default()
        }
    }

    /// Set the dimensions.
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Set the embedding type.
    pub fn with_embedding_type(mut self, embedding_type: EmbeddingType) -> Self {
        self.embedding_type = Some(embedding_type);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the maximum number of parallel calls.
    pub fn with_max_parallel_calls(mut self, max_parallel_calls: usize) -> Self {
        self.max_parallel_calls = Some(max_parallel_calls);
        self
    }

    /// Set telemetry configuration.
    pub fn with_telemetry(mut self, telemetry: TelemetrySettings) -> Self {
        self.telemetry = Some(telemetry);
        self
    }
}

/// Resolve an embedding model reference to an actual model instance.
fn resolve_embedding_model(model: EmbeddingModel) -> Result<Arc<dyn EmbeddingModelV4>, AIError> {
    crate::model::resolve_embedding_model(model).map_err(AIError::ProviderError)
}

/// Generate an embedding for a single text.
///
/// # Arguments
///
/// * `options` - The embedding options including model and text.
///
/// # Returns
///
/// An `EmbedResult` containing the embedding vector and usage.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{embed, EmbedOptions};
///
/// let result = embed(EmbedOptions {
///     model: "text-embedding-3-small".into(),
///     value: "Hello, world!".to_string(),
///     ..Default::default()
/// }).await?;
///
/// println!("Embedding dimension: {}", result.embedding.as_dense().map(|v| v.len()).unwrap_or(0));
/// ```
#[tracing::instrument(skip_all)]
pub async fn embed(options: EmbedOptions) -> Result<EmbedResult, AIError> {
    let model = resolve_embedding_model(options.model)?;

    // Store original value
    let original_value = options.value.clone();

    let mut call_options = EmbeddingModelV4CallOptions::new(vec![options.value]);

    if let Some(dimensions) = options.dimensions {
        call_options.dimensions = Some(dimensions);
    }
    if let Some(embedding_type) = options.embedding_type {
        call_options.embedding_type = Some(embedding_type);
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

    // Execute with retry
    let result = execute_embed_with_retry(&model, call_options, retry_config).await?;

    crate::logger::log_warnings(&crate::logger::LogWarningsOptions::new(
        result.warnings.clone(),
        model.provider(),
        model.model_id(),
    ));

    let raw_response = result.raw_response.clone();
    let embedding = result
        .embeddings
        .into_iter()
        .next()
        .ok_or(AIError::NoOutputGenerated)?;

    let mut embed_result = EmbedResult::new(original_value, embedding, result.usage);
    if let Some(raw) = raw_response {
        embed_result.raw_response = Some(raw);
    }
    Ok(embed_result)
}

/// Generate embeddings for multiple texts.
///
/// This function automatically splits large requests into smaller chunks if the model
/// has a limit on how many embeddings can be generated in a single call.
///
/// # Arguments
///
/// * `options` - The embedding options including model and texts.
///
/// # Returns
///
/// An `EmbedManyResult` containing the embedding vectors and usage.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{embed_many, EmbedManyOptions};
///
/// let result = embed_many(EmbedManyOptions {
///     model: "text-embedding-3-small".into(),
///     values: vec!["Hello".to_string(), "World".to_string()],
///     ..Default::default()
/// }).await?;
///
/// println!("Generated {} embeddings", result.embeddings.len());
/// ```
#[tracing::instrument(skip_all)]
pub async fn embed_many(options: EmbedManyOptions) -> Result<EmbedManyResult, AIError> {
    let model = resolve_embedding_model(options.model)?;

    // Get model limits
    let max_embeddings_per_call = model.max_embeddings_per_call();
    let supports_parallel_calls = model.supports_parallel_calls();

    // Store original values
    let original_values = options.values.clone();

    // Handle empty values case
    if original_values.is_empty() {
        return Ok(EmbedManyResult::new(vec![], vec![], EmbeddingUsage::new(0)));
    }

    // Build retry config
    let retry_config = options
        .max_retries
        .map(|max_retries| RetryConfig::new().with_max_retries(max_retries))
        .unwrap_or_default();

    // Determine max parallel calls
    let max_parallel = options.max_parallel_calls.unwrap_or(usize::MAX);

    // Check if we need to chunk
    let needs_chunking =
        max_embeddings_per_call > 0 && original_values.len() > max_embeddings_per_call;

    if !needs_chunking {
        // Single call case
        let mut call_options = EmbeddingModelV4CallOptions::new(original_values.clone());

        if let Some(dimensions) = options.dimensions {
            call_options.dimensions = Some(dimensions);
        }
        if let Some(embedding_type) = options.embedding_type {
            call_options.embedding_type = Some(embedding_type);
        }
        if let Some(signal) = options.abort_signal.clone() {
            call_options.abort_signal = Some(signal);
        }
        if let Some(ref headers) = options.headers {
            call_options.headers = Some(headers.clone());
        }
        if let Some(ref provider_opts) = options.provider_options {
            call_options.provider_options = Some(provider_opts.clone());
        }

        let result = execute_embed_with_retry(&model, call_options, retry_config).await?;

        crate::logger::log_warnings(&crate::logger::LogWarningsOptions::new(
            result.warnings.clone(),
            model.provider(),
            model.model_id(),
        ));

        let mut many_result =
            EmbedManyResult::new(original_values, result.embeddings, result.usage);
        if let Some(raw) = result.raw_response {
            many_result.raw_responses.push(raw);
        }
        return Ok(many_result);
    }

    // Chunking case: split values into chunks
    let chunks: Vec<Vec<String>> = original_values
        .chunks(max_embeddings_per_call)
        .map(<[std::string::String]>::to_vec)
        .collect();

    // Group chunks for parallel execution
    let parallel_group_size = if supports_parallel_calls {
        max_parallel
    } else {
        1
    };
    let parallel_groups: Vec<Vec<Vec<String>>> = chunks
        .chunks(parallel_group_size)
        .map(<[std::vec::Vec<std::string::String>]>::to_vec)
        .collect();

    // Process all chunks
    let mut all_embeddings: Vec<EmbeddingValue> = Vec::new();
    let mut total_tokens: u64 = 0;
    let mut all_raw_responses: Vec<serde_json::Value> = Vec::new();
    let mut all_warnings: Vec<vercel_ai_provider::Warning> = Vec::new();

    for parallel_group in parallel_groups {
        // Execute chunks in parallel
        let futures: Vec<_> = parallel_group
            .into_iter()
            .map(|chunk| {
                let model = model.clone();
                let dimensions = options.dimensions;
                let embedding_type = options.embedding_type;
                let abort_signal = options.abort_signal.clone();
                let headers = options.headers.clone();
                let provider_opts = options.provider_options.clone();
                let retry_config = retry_config.clone();

                async move {
                    let mut call_options = EmbeddingModelV4CallOptions::new(chunk);

                    if let Some(dimensions) = dimensions {
                        call_options.dimensions = Some(dimensions);
                    }
                    if let Some(embedding_type) = embedding_type {
                        call_options.embedding_type = Some(embedding_type);
                    }
                    if let Some(signal) = abort_signal {
                        call_options.abort_signal = Some(signal);
                    }
                    if let Some(ref headers) = headers {
                        call_options.headers = Some(headers.clone());
                    }
                    if let Some(ref provider_opts) = provider_opts {
                        call_options.provider_options = Some(provider_opts.clone());
                    }

                    execute_embed_with_retry(&model, call_options, retry_config).await
                }
            })
            .collect();

        // Wait for all parallel calls
        let results = futures::future::try_join_all(futures).await?;

        // Collect results
        for result in results {
            all_embeddings.extend(result.embeddings);
            total_tokens = total_tokens.saturating_add(result.usage.total_tokens);
            all_warnings.extend(result.warnings);
            if let Some(raw) = result.raw_response {
                all_raw_responses.push(raw);
            }
        }
    }

    crate::logger::log_warnings(&crate::logger::LogWarningsOptions::new(
        all_warnings,
        model.provider(),
        model.model_id(),
    ));

    // Build final result
    let usage = EmbeddingUsage::new(total_tokens);
    let mut result = EmbedManyResult::new(original_values, all_embeddings, usage);
    result.raw_responses = all_raw_responses;

    Ok(result)
}

/// Execute an embedding request with retry logic.
async fn execute_embed_with_retry(
    model: &Arc<dyn EmbeddingModelV4>,
    call_options: EmbeddingModelV4CallOptions,
    retry_config: RetryConfig,
) -> Result<EmbeddingModelV4EmbedResult, AIError> {
    let model = model.clone();

    with_retry(retry_config, None, || {
        let model = model.clone();
        let call_options = call_options.clone();
        async move { model.do_embed(call_options).await.map_err(AIError::from) }
    })
    .await
}

#[cfg(test)]
#[path = "generate.test.rs"]
mod tests;
