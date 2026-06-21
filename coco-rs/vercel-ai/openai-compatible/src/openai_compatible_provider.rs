use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::errors::LoadAPIKeyError;
use vercel_ai_provider::errors::NoSuchModelError;
use vercel_ai_provider::provider::v4::FromEnvProvider;
use vercel_ai_provider_utils::ResponseHandler;
use vercel_ai_provider_utils::load_api_key;

use crate::chat::OpenAICompatibleChatLanguageModel;
use crate::completion::OpenAICompatibleCompletionLanguageModel;
use crate::embedding::OpenAICompatibleEmbeddingModel;
use crate::image::OpenAICompatibleImageModel;
use crate::openai_compatible_config::OpenAICompatibleConfig;
use crate::openai_compatible_error::OpenAICompatibleFailedResponseHandler;
use crate::openai_compatible_provider_settings::OpenAICompatibleProviderSettings;
use crate::provider_options::PromptTokensTotalSemantics;
use crate::provider_profile::OpenAICompatibleProviderProfile;

/// OpenAI-compatible multi-model provider.
///
/// Implements `ProviderV4` and provides access to Chat, Completion,
/// Embedding, and Image models for any OpenAI-compatible API.
pub struct OpenAICompatibleProvider {
    provider_name: String,
    base_url: String,
    headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    query_params: Option<HashMap<String, String>>,
    client: Option<Arc<reqwest::Client>>,
    include_usage: bool,
    supports_structured_outputs: bool,
    transform_request_body:
        Option<Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>>,
    metadata_extractor: Option<Arc<dyn crate::metadata_extractor::MetadataExtractor>>,
    error_handler: Option<Arc<dyn ResponseHandler<AISdkError>>>,
    prompt_tokens_total_semantics: PromptTokensTotalSemantics,
    provider_profile: OpenAICompatibleProviderProfile,
    full_url: Option<bool>,
}

impl OpenAICompatibleProvider {
    /// Create a new provider from settings.
    pub fn new(settings: OpenAICompatibleProviderSettings) -> Self {
        let provider_name = settings.name.unwrap_or_else(|| "openai-compatible".into());
        let api_key_env_var = settings
            .api_key_env_var
            .unwrap_or_else(|| "OPENAI_API_KEY".into());
        let api_key_description = settings
            .api_key_description
            .unwrap_or_else(|| "OpenAI-compatible".into());
        let base_url = settings
            .base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".into())
            .trim_end_matches('/')
            .to_string();

        let api_key = settings.api_key;
        let custom_headers = settings.headers.unwrap_or_default();
        let env_var = api_key_env_var;
        let desc = api_key_description;

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let mut h = HashMap::new();

            // API key — loaded lazily per request
            let key = load_api_key(api_key.as_deref(), &env_var, &desc).unwrap_or_default();
            if !key.is_empty() {
                h.insert("Authorization".into(), format!("Bearer {key}"));
            }

            // User-Agent suffix: ai-sdk/openai-compatible/VERSION
            let version = env!("CARGO_PKG_VERSION");
            let ua = format!("ai-sdk/openai-compatible/{version}");
            h.entry("User-Agent".into())
                .and_modify(|existing| {
                    existing.push(' ');
                    existing.push_str(&ua);
                })
                .or_insert(ua);

            // Merge custom headers (custom overrides default)
            for (k, v) in &custom_headers {
                h.insert(k.clone(), v.clone());
            }

            h
        });

        let provider_profile = settings.provider_profile.unwrap_or_default();

        Self {
            provider_name,
            base_url,
            headers,
            query_params: settings.query_params,
            client: settings.client,
            include_usage: settings
                .include_usage
                .unwrap_or_else(|| provider_profile.default_include_usage()),
            supports_structured_outputs: settings.supports_structured_outputs.unwrap_or(false),
            transform_request_body: settings.transform_request_body,
            metadata_extractor: settings.metadata_extractor,
            error_handler: settings.error_handler,
            prompt_tokens_total_semantics: settings.prompt_tokens_total_semantics,
            provider_profile,
            full_url: settings.full_url,
        }
    }

    fn make_config(&self, sub_provider: &str) -> Arc<OpenAICompatibleConfig> {
        let error_handler: Arc<dyn ResponseHandler<AISdkError>> =
            self.error_handler.clone().unwrap_or_else(|| {
                Arc::new(OpenAICompatibleFailedResponseHandler::new(
                    &self.provider_name,
                ))
            });

        Arc::new(OpenAICompatibleConfig {
            provider: format!("{}.{sub_provider}", self.provider_name),
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            query_params: self.query_params.clone(),
            client: self.client.clone(),
            include_usage: self.include_usage,
            supports_structured_outputs: self.supports_structured_outputs,
            transform_request_body: self.transform_request_body.clone(),
            metadata_extractor: self.metadata_extractor.clone(),
            supported_urls: None,
            error_handler,
            prompt_tokens_total_semantics: self.prompt_tokens_total_semantics,
            provider_profile: self.provider_profile,
            full_url: self.full_url,
        })
    }

    /// Get a Chat Completions language model.
    pub fn chat(&self, model_id: &str) -> OpenAICompatibleChatLanguageModel {
        OpenAICompatibleChatLanguageModel::new(model_id, self.make_config("chat"))
    }

    /// Get a legacy Completions language model.
    pub fn completion(&self, model_id: &str) -> OpenAICompatibleCompletionLanguageModel {
        OpenAICompatibleCompletionLanguageModel::new(model_id, self.make_config("completion"))
    }

    /// Get an embedding model.
    pub fn embedding(&self, model_id: &str) -> OpenAICompatibleEmbeddingModel {
        OpenAICompatibleEmbeddingModel::new(model_id, self.make_config("embedding"))
    }

    /// Get an image model.
    pub fn image(&self, model_id: &str) -> OpenAICompatibleImageModel {
        OpenAICompatibleImageModel::new(model_id, self.make_config("image"))
    }
}

#[async_trait]
impl ProviderV4 for OpenAICompatibleProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    /// Default language model uses the Chat Completions API (no Responses API).
    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.chat(model_id)))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.embedding(model_id)))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.image(model_id)))
    }
}

impl FromEnvProvider for OpenAICompatibleProvider {
    fn from_env() -> Result<Self, LoadAPIKeyError> {
        // Try OPENAI_API_KEY by default
        load_api_key(None, "OPENAI_API_KEY", "OpenAI-compatible")?;
        Ok(Self::new(OpenAICompatibleProviderSettings::default()))
    }
}

/// Create an OpenAI-compatible provider with custom settings.
pub fn create_openai_compatible(
    settings: OpenAICompatibleProviderSettings,
) -> OpenAICompatibleProvider {
    OpenAICompatibleProvider::new(settings)
}

#[cfg(test)]
#[path = "openai_compatible_provider.test.rs"]
mod tests;
