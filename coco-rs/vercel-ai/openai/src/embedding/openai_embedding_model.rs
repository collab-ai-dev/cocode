use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::EmbeddingModelV4CallOptions;
use vercel_ai_provider::EmbeddingModelV4EmbedResult;
use vercel_ai_provider::EmbeddingUsage;
use vercel_ai_provider::EmbeddingValue;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use crate::openai_config::OpenAIConfig;
use crate::openai_error::OpenAIFailedResponseHandler;

use super::openai_embedding_api::OpenAIEmbeddingResponse;
use super::openai_embedding_options::extract_embedding_options;

/// OpenAI Embedding model.
pub struct OpenAIEmbeddingModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAIEmbeddingModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl EmbeddingModelV4 for OpenAIEmbeddingModel {
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
        // Validate max embedding values per call
        if options.values.len() > self.max_embeddings_per_call() {
            return Err(AISdkError::new(format!(
                "Too many values for a single embedding call. The {} model supports at most {} values per call, but {} values were provided.",
                self.model_id,
                self.max_embeddings_per_call(),
                options.values.len()
            )));
        }

        let openai_opts = extract_embedding_options(&options.provider_options);

        let mut body = json!({
            "model": self.model_id,
            "input": options.values,
            "encoding_format": "float",
        });

        // Use provider option dimensions, fall back to call option dimensions
        let dimensions = openai_opts.dimensions.or(options.dimensions);
        if let Some(dims) = dimensions {
            body["dimensions"] = json!(dims);
        }

        if let Some(ref user) = openai_opts.user {
            body["user"] = serde_json::Value::String(user.clone());
        }

        let url = self.config.url("/embeddings");
        let headers = self.config.get_headers();

        let response: OpenAIEmbeddingResponse = post_json_to_api_with_client(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            OpenAIFailedResponseHandler,
            options.abort_signal,
            self.config.client.clone(),
        )
        .await?;

        let raw_response = serde_json::to_value(&response).ok();

        let embeddings: Vec<EmbeddingValue> = response
            .data
            .into_iter()
            .map(|d| EmbeddingValue::Dense {
                vector: d.embedding,
            })
            .collect();

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

        Ok(EmbeddingModelV4EmbedResult {
            embeddings,
            usage,
            warnings: Vec::new(),
            provider_metadata: None,
            raw_response,
        })
    }
}

#[cfg(test)]
#[path = "openai_embedding_model.test.rs"]
mod tests;
