use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::errors::LoadAPIKeyError;
use vercel_ai_provider::errors::NoSuchModelError;
use vercel_ai_provider::provider::v4::FromEnvProvider;
use vercel_ai_provider_utils::load_api_key;

use crate::chat::XaiChatLanguageModel;
use crate::image::XaiImageModel;
use crate::responses::XaiResponsesLanguageModel;
use crate::speech::XaiSpeechModel;
use crate::transcription::XaiTranscriptionModel;
use crate::video::XaiVideoModel;
use crate::xai_config::XaiConfig;

/// Default xAI API base URL.
const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
/// Environment variable holding the xAI API key.
const API_KEY_ENV_VAR: &str = "XAI_API_KEY";

/// Settings for constructing an [`XaiProvider`].
#[derive(Debug, Default)]
pub struct XaiProviderSettings {
    /// Base URL for xAI API calls. Defaults to `https://api.x.ai/v1`.
    pub base_url: Option<String>,
    /// API key. Falls back to the `XAI_API_KEY` environment variable.
    pub api_key: Option<String>,
    /// Custom headers to include in requests.
    pub headers: Option<HashMap<String, String>>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// xAI (Grok) provider. Exposes chat language models over the Chat Completions
/// API.
///
/// Mirrors `XaiProvider` from `@ai-sdk/xai`, scoped to the Chat Completions
/// surface. The upstream `languageModel` default routes to the Responses API;
/// here `language_model()` maps to Chat Completions (the coco-rs convention,
/// matching the sibling `vercel-ai-groq` crate).
pub struct XaiProvider {
    base_url: String,
    headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    client: Option<Arc<reqwest::Client>>,
}

impl XaiProvider {
    /// Create a new provider from settings.
    pub fn new(settings: XaiProviderSettings) -> Self {
        let base_url = settings
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let api_key = settings.api_key;
        let custom_headers = settings.headers.unwrap_or_default();

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let mut h = HashMap::new();

            let key = load_api_key(api_key.as_deref(), API_KEY_ENV_VAR, "xAI").unwrap_or_default();
            if !key.is_empty() {
                h.insert("Authorization".into(), format!("Bearer {key}"));
            }

            let version = env!("CARGO_PKG_VERSION");
            let ua = format!("ai-sdk/xai/{version}");
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

    fn make_config(&self, sub_provider: &str) -> Arc<XaiConfig> {
        Arc::new(XaiConfig {
            provider: format!("xai.{sub_provider}"),
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            client: self.client.clone(),
        })
    }

    /// Get an xAI chat language model.
    pub fn chat(&self, model_id: &str) -> XaiChatLanguageModel {
        XaiChatLanguageModel::new(model_id, self.make_config("chat"))
    }

    /// Get an xAI Responses API language model.
    ///
    /// Opt-in: `language_model()` stays on Chat Completions (the coco-rs
    /// convention); the Responses surface is reached explicitly through this
    /// constructor (sub-provider id `xai.responses`).
    pub fn responses(&self, model_id: &str) -> XaiResponsesLanguageModel {
        XaiResponsesLanguageModel::new(model_id, self.make_config("responses"))
    }

    /// Get an xAI image generation model (sub-provider id `xai.image`).
    pub fn image(&self, model_id: &str) -> XaiImageModel {
        XaiImageModel::new(model_id, self.make_config("image"))
    }

    /// Get an xAI video generation model (sub-provider id `xai.video`).
    pub fn video(&self, model_id: &str) -> XaiVideoModel {
        XaiVideoModel::new(model_id, self.make_config("video"))
    }

    /// Get an xAI speech (text-to-speech) model (sub-provider id
    /// `xai.speech`). The `/tts` endpoint takes no model field — upstream
    /// exposes `speech()` without a model id and pins it to `""`; pass `""`
    /// for parity. The id is carried only as response metadata.
    pub fn speech(&self, model_id: &str) -> XaiSpeechModel {
        XaiSpeechModel::new(model_id, self.make_config("speech"))
    }

    /// Get an xAI batch transcription (speech-to-text) model (sub-provider
    /// id `xai.transcription`). The `/stt` endpoint takes no model field —
    /// upstream exposes `transcription()` without a model id and pins it to
    /// `""`; pass `""` for parity. The id is carried only as response
    /// metadata.
    pub fn transcription(&self, model_id: &str) -> XaiTranscriptionModel {
        XaiTranscriptionModel::new(model_id, self.make_config("transcription"))
    }
}

#[async_trait]
impl ProviderV4 for XaiProvider {
    fn provider(&self) -> &str {
        "xai"
    }

    /// The default language model uses the Chat Completions API.
    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.chat(model_id)))
    }

    /// xAI does not expose embedding models.
    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "embeddingModel",
        ))
    }

    /// Get an xAI image generation model.
    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.image(model_id)))
    }

    /// Get an xAI video generation model.
    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.video(model_id)))
    }

    /// Get an xAI speech (text-to-speech) model.
    fn speech_model(&self, model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.speech(model_id)))
    }

    /// Get an xAI batch transcription (speech-to-text) model.
    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.transcription(model_id)))
    }
}

impl FromEnvProvider for XaiProvider {
    fn from_env() -> Result<Self, LoadAPIKeyError> {
        load_api_key(None, API_KEY_ENV_VAR, "xAI")?;
        Ok(Self::new(XaiProviderSettings::default()))
    }
}

/// Create an xAI provider with custom settings. Mirrors `createXai`.
pub fn create_xai(settings: XaiProviderSettings) -> XaiProvider {
    XaiProvider::new(settings)
}

#[cfg(test)]
#[path = "xai_provider.test.rs"]
mod tests;
