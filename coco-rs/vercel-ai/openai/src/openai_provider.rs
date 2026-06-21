use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::errors::LoadAPIKeyError;
use vercel_ai_provider::errors::NoSuchModelError;
use vercel_ai_provider::provider::v4::FromEnvProvider;
use vercel_ai_provider_utils::load_api_key;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::chat::OpenAIChatLanguageModel;
use crate::completion::OpenAICompletionLanguageModel;
use crate::embedding::OpenAIEmbeddingModel;
use crate::image::OpenAIImageModel;
use crate::openai_auth::CHATGPT_CODEX_BASE_URL;
use crate::openai_auth::HDR_CHATGPT_ACCOUNT_ID;
use crate::openai_auth::HDR_ORIGINATOR;
use crate::openai_auth::OpenAIAuth;
use crate::openai_config::OpenAIConfig;
use crate::openai_config::ResponsesStorePolicy;
use crate::responses::OpenAIResponsesLanguageModel;
use crate::speech::OpenAISpeechModel;
use crate::transcription::OpenAITranscriptionModel;

/// Settings for creating an OpenAI provider.
#[derive(Default)]
pub struct OpenAIProviderSettings {
    /// Base URL. Default: `https://api.openai.com/v1` for `ApiKey`,
    /// `https://chatgpt.com/backend-api/codex` for `ChatGptSubscription`.
    pub base_url: Option<String>,
    /// How to authenticate each request. Default: `ApiKey(None)` (reads
    /// `OPENAI_API_KEY`).
    pub auth: OpenAIAuth,
    /// OpenAI organization ID.
    pub organization: Option<String>,
    /// OpenAI project ID.
    pub project: Option<String>,
    /// Custom headers to include in every request.
    pub headers: Option<HashMap<String, String>>,
    /// Provider name (default: "openai").
    pub name: Option<String>,
    /// Shared HTTP client.
    pub client: Option<Arc<reqwest::Client>>,
    /// When `true`, `base_url` is the complete endpoint URL — no API path
    /// suffix is appended. Default (`None`): auto-detect duplicate suffixes.
    pub full_url: Option<bool>,
    /// Policy for the Responses `store` field on reasoning models (see
    /// [`ResponsesStorePolicy`]). Default `ServerDefault`.
    pub reasoning_store: ResponsesStorePolicy,
}

/// OpenAI multi-model provider.
///
/// Implements `ProviderV4` and provides access to Chat, Responses,
/// Completion, Embedding, and Image models.
pub struct OpenAIProvider {
    provider_name: String,
    base_url: String,
    headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    client: Option<Arc<reqwest::Client>>,
    full_url: Option<bool>,
    chatgpt_subscription: bool,
    reasoning_store: ResponsesStorePolicy,
}

impl OpenAIProvider {
    /// Create a new provider from settings.
    pub fn new(settings: OpenAIProviderSettings) -> Self {
        let provider_name = settings.name.unwrap_or_else(|| "openai".into());
        let chatgpt_subscription = settings.auth.is_chatgpt_subscription();
        let reasoning_store = settings.reasoning_store;
        // Late host resolution (mirrors codex `to_api_provider`): a
        // subscription with no explicit base defaults to the codex backend.
        let default_base = if chatgpt_subscription {
            CHATGPT_CODEX_BASE_URL
        } else {
            "https://api.openai.com/v1"
        };
        let base_url = settings
            .base_url
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| default_base.into());
        let base_url = without_trailing_slash(&base_url).to_string();

        let auth = settings.auth;
        let organization = settings.organization;
        let project = settings.project;
        let custom_headers = settings.headers.unwrap_or_default();

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let mut h = HashMap::new();

            match &auth {
                OpenAIAuth::ApiKey(api_key) => {
                    // API key — loaded lazily per request.
                    match load_api_key(api_key.as_deref(), "OPENAI_API_KEY", "OpenAI") {
                        Ok(key) if !key.is_empty() => {
                            h.insert("Authorization".into(), format!("Bearer {key}"));
                        }
                        Err(e) => tracing::warn!("OpenAI API key not configured: {e}"),
                        _ => {}
                    }
                    if let Some(ref org) = organization {
                        h.insert("OpenAI-Organization".into(), org.clone());
                    }
                    if let Some(ref proj) = project {
                        h.insert("OpenAI-Project".into(), proj.clone());
                    }
                }
                OpenAIAuth::ChatGptSubscription { creds, originator } => {
                    h.insert(HDR_ORIGINATOR.into(), originator.to_string());
                    match creds() {
                        Some(c) => {
                            h.insert("Authorization".into(), format!("Bearer {}", c.access_token));
                            if let Some(account_id) = c.account_id {
                                h.insert(HDR_CHATGPT_ACCOUNT_ID.into(), account_id);
                            }
                        }
                        None => tracing::warn!(
                            "ChatGPT subscription not logged in — run `coco login openai`"
                        ),
                    }
                }
            }

            // Merge custom headers (custom overrides default).
            for (k, v) in &custom_headers {
                h.insert(k.clone(), v.clone());
            }

            h
        });

        Self {
            provider_name,
            base_url,
            headers,
            client: settings.client,
            full_url: settings.full_url,
            chatgpt_subscription,
            reasoning_store,
        }
    }

    fn make_config(&self, sub_provider: &str) -> Arc<OpenAIConfig> {
        Arc::new(OpenAIConfig {
            provider: format!("{}.{sub_provider}", self.provider_name),
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            client: self.client.clone(),
            full_url: self.full_url,
            chatgpt_subscription: self.chatgpt_subscription,
            reasoning_store: self.reasoning_store,
        })
    }

    /// Get a Chat Completions language model.
    pub fn chat(&self, model_id: &str) -> OpenAIChatLanguageModel {
        OpenAIChatLanguageModel::new(model_id, self.make_config("chat"))
    }

    /// Get a Responses API language model.
    pub fn responses(&self, model_id: &str) -> OpenAIResponsesLanguageModel {
        OpenAIResponsesLanguageModel::new(model_id, self.make_config("responses"))
    }

    /// Get a legacy Completions language model.
    pub fn completion(&self, model_id: &str) -> OpenAICompletionLanguageModel {
        OpenAICompletionLanguageModel::new(model_id, self.make_config("completion"))
    }

    /// Get an embedding model.
    pub fn embedding(&self, model_id: &str) -> OpenAIEmbeddingModel {
        OpenAIEmbeddingModel::new(model_id, self.make_config("embedding"))
    }

    /// Get an image model.
    pub fn image(&self, model_id: &str) -> OpenAIImageModel {
        OpenAIImageModel::new(model_id, self.make_config("image"))
    }

    /// Get a speech (text-to-speech) model.
    pub fn speech(&self, model_id: &str) -> OpenAISpeechModel {
        OpenAISpeechModel::new(model_id, self.make_config("speech"))
    }

    /// Get a transcription (speech-to-text) model.
    pub fn transcription(&self, model_id: &str) -> OpenAITranscriptionModel {
        OpenAITranscriptionModel::new(model_id, self.make_config("transcription"))
    }
}

#[async_trait]
impl ProviderV4 for OpenAIProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    /// Default language model uses the Responses API.
    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.responses(model_id)))
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

    fn speech_model(&self, model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.speech(model_id)))
    }

    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.transcription(model_id)))
    }
}

impl FromEnvProvider for OpenAIProvider {
    fn from_env() -> Result<Self, LoadAPIKeyError> {
        // Validate that the key exists
        load_api_key(None, "OPENAI_API_KEY", "OpenAI")?;
        Ok(Self::new(OpenAIProviderSettings::default()))
    }
}

/// Create an OpenAI provider with custom settings.
pub fn create_openai(settings: OpenAIProviderSettings) -> OpenAIProvider {
    OpenAIProvider::new(settings)
}

/// Create a default OpenAI provider using env vars.
pub fn openai() -> OpenAIProvider {
    OpenAIProvider::new(OpenAIProviderSettings::default())
}

#[cfg(test)]
#[path = "openai_provider.test.rs"]
mod tests;
