use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::errors::LoadAPIKeyError;
use vercel_ai_provider::errors::NoSuchModelError;
use vercel_ai_provider::provider::v4::FromEnvProvider;
use vercel_ai_provider_utils::load_api_key;

use crate::chat::GroqChatLanguageModel;
use crate::groq_config::GroqConfig;
use crate::transcription::GroqTranscriptionModel;

/// Default Groq API base URL.
const DEFAULT_BASE_URL: &str = "https://api.groq.com/openai/v1";
/// Environment variable holding the Groq API key.
const API_KEY_ENV_VAR: &str = "GROQ_API_KEY";

/// Settings for constructing a [`GroqProvider`].
#[derive(Debug, Default)]
pub struct GroqProviderSettings {
    /// Base URL for Groq API calls. Defaults to `https://api.groq.com/openai/v1`.
    pub base_url: Option<String>,
    /// API key. Falls back to the `GROQ_API_KEY` environment variable.
    pub api_key: Option<String>,
    /// Custom headers to include in requests.
    pub headers: Option<HashMap<String, String>>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// Groq provider. Exposes chat language models and transcription models.
///
/// Mirrors `GroqProvider` from `@ai-sdk/groq`.
pub struct GroqProvider {
    base_url: String,
    headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    client: Option<Arc<reqwest::Client>>,
}

impl GroqProvider {
    /// Create a new provider from settings.
    pub fn new(settings: GroqProviderSettings) -> Self {
        let base_url = settings
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let api_key = settings.api_key;
        let custom_headers = settings.headers.unwrap_or_default();

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let mut h = HashMap::new();

            let key = load_api_key(api_key.as_deref(), API_KEY_ENV_VAR, "Groq").unwrap_or_default();
            if !key.is_empty() {
                h.insert("Authorization".into(), format!("Bearer {key}"));
            }

            let version = env!("CARGO_PKG_VERSION");
            let ua = format!("ai-sdk/groq/{version}");
            h.entry("User-Agent".into())
                .and_modify(|existing| {
                    existing.push(' ');
                    existing.push_str(&ua);
                })
                .or_insert(ua);

            for (k, v) in &custom_headers {
                h.insert(k.clone(), v.clone());
            }

            h
        });

        Self {
            base_url,
            headers,
            client: settings.client,
        }
    }

    fn make_config(&self, sub_provider: &str) -> Arc<GroqConfig> {
        Arc::new(GroqConfig {
            provider: format!("groq.{sub_provider}"),
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            client: self.client.clone(),
        })
    }

    /// Get a Groq chat language model.
    pub fn chat(&self, model_id: &str) -> GroqChatLanguageModel {
        GroqChatLanguageModel::new(model_id, self.make_config("chat"))
    }

    /// Get a Groq transcription model.
    pub fn transcription(&self, model_id: &str) -> GroqTranscriptionModel {
        GroqTranscriptionModel::new(model_id, self.make_config("transcription"))
    }
}

#[async_trait]
impl ProviderV4 for GroqProvider {
    fn provider(&self) -> &str {
        "groq"
    }

    /// The default language model uses the Chat Completions API.
    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.chat(model_id)))
    }

    /// Groq does not provide embedding models.
    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "embeddingModel",
        ))
    }

    /// Groq does not provide image models.
    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "imageModel",
        ))
    }

    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.transcription(model_id)))
    }
}

impl FromEnvProvider for GroqProvider {
    fn from_env() -> Result<Self, LoadAPIKeyError> {
        load_api_key(None, API_KEY_ENV_VAR, "Groq")?;
        Ok(Self::new(GroqProviderSettings::default()))
    }
}

/// Create a Groq provider with custom settings. Mirrors `createGroq`.
pub fn create_groq(settings: GroqProviderSettings) -> GroqProvider {
    GroqProvider::new(settings)
}

#[cfg(test)]
#[path = "groq_provider.test.rs"]
mod tests;
