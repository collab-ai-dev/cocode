use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::EmbeddingModelV4CallOptions;
use vercel_ai_provider::EmbeddingModelV4EmbedResult;
use vercel_ai_provider::EmbeddingUsage;
use vercel_ai_provider::EmbeddingValue;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use crate::openai_compatible_config::OpenAICompatibleConfig;

use super::openai_compatible_embedding_api::OpenAICompatibleEmbeddingResponse;
use super::openai_compatible_embedding_options::extract_embedding_options;

/// OpenAI-compatible Embedding model.
pub struct OpenAICompatibleEmbeddingModel {
    model_id: String,
    config: Arc<OpenAICompatibleConfig>,
}

impl OpenAICompatibleEmbeddingModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAICompatibleConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl EmbeddingModelV4 for OpenAICompatibleEmbeddingModel {
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
        // Validate embedding count
        if options.values.len() > self.max_embeddings_per_call() {
            return Err(AISdkError::new(format!(
                "Too many values for a single call. The {} model can only embed up to {} values per call, but {} values were provided.",
                self.model_id,
                self.max_embeddings_per_call(),
                options.values.len()
            )));
        }

        let provider_name = self.config.provider_options_name();
        let compat_opts = extract_embedding_options(&options.provider_options, provider_name);

        let mut body = json!({
            "model": self.model_id,
            "input": options.values,
            "encoding_format": "float",
        });

        // Use provider option dimensions, fall back to call option dimensions
        let dimensions = compat_opts.dimensions.or(options.dimensions);
        if let Some(dims) = dimensions {
            body["dimensions"] = json!(dims);
        }

        if let Some(ref user) = compat_opts.user {
            body["user"] = serde_json::Value::String(user.clone());
        }

        // Apply request body transform
        let body = self.config.transform_body(body);

        let url = self.config.url("/embeddings");
        let headers = self.config.get_headers();

        let response: OpenAICompatibleEmbeddingResponse = post_json_to_api_with_client(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            self.config.error_handler.clone(),
            options.abort_signal,
            self.config.client.clone(),
        )
        .await?;

        // Serialize response before consuming it
        let raw_response = serde_json::to_value(&response).ok();

        // Convert provider_metadata from response
        let provider_metadata = response
            .provider_metadata
            .map(|pm| ProviderMetadata::from_map(pm.into_iter().collect()));

        let usage = EmbeddingUsage {
            prompt_tokens: response
                .usage
                .as_ref()
                .and_then(|u| u.prompt_tokens)
                .unwrap_or(0),
            total_tokens: response
                .usage
                .as_ref()
                .and_then(|u| u.total_tokens)
                .unwrap_or(0),
        };

        let embeddings: Vec<EmbeddingValue> = response
            .data
            .into_iter()
            .map(|d| EmbeddingValue::Dense {
                vector: d.embedding,
            })
            .collect();

        Ok(EmbeddingModelV4EmbedResult {
            embeddings,
            usage,
            warnings: Vec::new(),
            provider_metadata,
            raw_response,
        })
    }
}

#[cfg(test)]
#[path = "openai_compatible_embedding_model.test.rs"]
mod tests;
