//! Google Generative AI embedding model implementation.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::EmbeddingModelV4CallOptions;
use vercel_ai_provider::EmbeddingModelV4EmbedResult;
use vercel_ai_provider::EmbeddingUsage;
use vercel_ai_provider::EmbeddingValue;

use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::combine_headers;
use vercel_ai_provider_utils::post_json_to_api_with_client;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::get_model_path::get_model_path;
use crate::google_error::GoogleFailedResponseHandler;
use crate::google_generative_ai_embedding_options::GoogleEmbeddingModelOptions;

/// Configuration for the Google Generative AI embedding model.
pub struct GoogleGenerativeAIEmbeddingModelConfig {
    /// Provider identifier string.
    pub provider: String,
    /// Base URL for the API.
    pub base_url: String,
    /// Function to generate request headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// Google Generative AI embedding model.
pub struct GoogleGenerativeAIEmbeddingModel {
    model_id: String,
    config: GoogleGenerativeAIEmbeddingModelConfig,
}

impl GoogleGenerativeAIEmbeddingModel {
    /// Create a new Google Generative AI embedding model.
    pub fn new(
        model_id: impl Into<String>,
        config: GoogleGenerativeAIEmbeddingModelConfig,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Parse provider options from call options.
    fn parse_provider_options(
        &self,
        options: &EmbeddingModelV4CallOptions,
    ) -> GoogleEmbeddingModelOptions {
        let Some(ref provider_options) = options.provider_options else {
            return GoogleEmbeddingModelOptions::default();
        };

        let opts_map = provider_options
            .0
            .get("google")
            .or_else(|| provider_options.0.get("vertex"));

        let Some(opts_map) = opts_map else {
            return GoogleEmbeddingModelOptions::default();
        };

        // Convert HashMap<String, Value> to Value, then deserialize.
        let Ok(opts_value) = serde_json::to_value(opts_map) else {
            return GoogleEmbeddingModelOptions::default();
        };

        serde_json::from_value(opts_value).unwrap_or_default()
    }

    /// Build content parts for a single embedding value, merging text with multimodal content.
    fn build_content_parts(text: &str, multimodal_parts: Option<&Vec<Value>>) -> Value {
        let text_part = if !text.is_empty() {
            vec![json!({ "text": text })]
        } else {
            vec![]
        };
        match multimodal_parts {
            Some(parts) => {
                let mut all_parts = text_part;
                all_parts.extend(parts.iter().cloned());
                json!(all_parts)
            }
            None => json!([{ "text": text }]),
        }
    }
}

/// Google embedding response (single).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEmbedContentResponse {
    embedding: GoogleEmbedding,
}

/// Google batch embedding response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleBatchEmbedContentsResponse {
    embeddings: Vec<GoogleEmbedding>,
}

/// A single embedding value.
#[derive(Debug, Deserialize)]
struct GoogleEmbedding {
    values: Vec<f32>,
}

#[async_trait]
impl EmbeddingModelV4 for GoogleGenerativeAIEmbeddingModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_embeddings_per_call(&self) -> usize {
        2048
    }

    fn supports_parallel_calls(&self) -> bool {
        true
    }

    async fn do_embed(
        &self,
        options: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4EmbedResult, AISdkError> {
        let provider_opts = self.parse_provider_options(&options);

        // Validate: too many values
        if options.values.len() > self.max_embeddings_per_call() {
            return Err(AISdkError::new(format!(
                "Too many values for a single embedding call. \
                 The {} model supports at most {} embeddings per call, but {} values were provided. \
                 Provider: {}",
                self.model_id,
                self.max_embeddings_per_call(),
                options.values.len(),
                self.config.provider,
            )));
        }

        let multimodal_content = provider_opts.content.as_ref();

        // Validate: multimodal content length must match values length
        if let Some(content) = multimodal_content
            && content.len() != options.values.len()
        {
            return Err(AISdkError::new(format!(
                "The number of multimodal content entries ({}) must match the number of values ({}).",
                content.len(),
                options.values.len(),
            )));
        }

        let headers = combine_headers(vec![Some((self.config.headers)()), options.headers.clone()]);

        let model_path = get_model_path(&self.model_id);

        // Determine output dimensionality.
        let output_dimensionality = options
            .dimensions
            .map(|d| d as u32)
            .or(provider_opts.output_dimensionality);

        if options.values.len() == 1 {
            // Single embedding: use embedContent endpoint.
            let url = format!(
                "{}/{}:embedContent",
                without_trailing_slash(&self.config.base_url),
                model_path
            );

            let multimodal_parts = multimodal_content
                .and_then(|c| c.first())
                .and_then(|v| v.as_ref());
            let parts = Self::build_content_parts(&options.values[0], multimodal_parts);

            let mut body = json!({
                "model": model_path,
                "content": {
                    "parts": parts
                }
            });

            if let Some(dim) = output_dimensionality {
                body["outputDimensionality"] = json!(dim);
            }
            if let Some(ref task_type) = provider_opts.task_type
                && let Ok(val) = serde_json::to_value(task_type)
            {
                body["taskType"] = val;
            }

            let response: GoogleEmbedContentResponse = post_json_to_api_with_client(
                &url,
                Some(headers),
                &body,
                JsonResponseHandler::new(),
                GoogleFailedResponseHandler,
                options.abort_signal.clone(),
                self.config.client.clone(),
            )
            .await?;

            Ok(EmbeddingModelV4EmbedResult {
                embeddings: vec![EmbeddingValue::Dense {
                    vector: response.embedding.values,
                }],
                usage: EmbeddingUsage {
                    prompt_tokens: 0,
                    total_tokens: 0,
                },
                warnings: Vec::new(),
                provider_metadata: None,
                raw_response: None,
            })
        } else {
            // Batch embedding: use batchEmbedContents endpoint.
            let url = format!(
                "{}/{}:batchEmbedContents",
                without_trailing_slash(&self.config.base_url),
                model_path
            );

            let requests: Vec<Value> = options
                .values
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let multimodal_parts = multimodal_content
                        .and_then(|c| c.get(index))
                        .and_then(|v| v.as_ref());
                    let parts = Self::build_content_parts(text, multimodal_parts);

                    let mut req = json!({
                        "model": model_path,
                        "content": {
                            "role": "user",
                            "parts": parts
                        }
                    });
                    if let Some(dim) = output_dimensionality {
                        req["outputDimensionality"] = json!(dim);
                    }
                    if let Some(ref task_type) = provider_opts.task_type
                        && let Ok(val) = serde_json::to_value(task_type)
                    {
                        req["taskType"] = val;
                    }
                    req
                })
                .collect();

            let body = json!({ "requests": requests });

            let response: GoogleBatchEmbedContentsResponse = post_json_to_api_with_client(
                &url,
                Some(headers),
                &body,
                JsonResponseHandler::new(),
                GoogleFailedResponseHandler,
                options.abort_signal.clone(),
                self.config.client.clone(),
            )
            .await?;

            let embeddings = response
                .embeddings
                .into_iter()
                .map(|e| EmbeddingValue::Dense { vector: e.values })
                .collect();

            Ok(EmbeddingModelV4EmbedResult {
                embeddings,
                usage: EmbeddingUsage {
                    prompt_tokens: 0,
                    total_tokens: 0,
                },
                warnings: Vec::new(),
                provider_metadata: None,
                raw_response: None,
            })
        }
    }
}

#[cfg(test)]
#[path = "google_generative_ai_embedding_model.test.rs"]
mod tests;
