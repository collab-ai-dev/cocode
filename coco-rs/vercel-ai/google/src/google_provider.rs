//! Google Generative AI provider factory.
//!
//! Creates instances of Google language, embedding, image, and video models.

use std::collections::HashMap;
use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::VideoModelV4;

use regex::Regex;
use vercel_ai_provider_utils::load_api_key;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::google_generative_ai_embedding_model::GoogleGenerativeAIEmbeddingModel;
use crate::google_generative_ai_embedding_model::GoogleGenerativeAIEmbeddingModelConfig;
use crate::google_generative_ai_image_model::GoogleGenerativeAIImageModel;
use crate::google_generative_ai_image_model::GoogleGenerativeAIImageModelConfig;
use crate::google_generative_ai_image_settings::GoogleGenerativeAIImageSettings;
use crate::google_generative_ai_language_model::GoogleGenerativeAILanguageModel;
use crate::google_generative_ai_language_model::GoogleGenerativeAILanguageModelConfig;
use crate::google_generative_ai_video_model::GoogleGenerativeAIVideoModel;
use crate::google_generative_ai_video_model::GoogleGenerativeAIVideoModelConfig;

/// Default base URL for the Google Generative AI API.
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default environment variable for the Google API key.
const DEFAULT_API_KEY_ENV: &str = "GOOGLE_GENERATIVE_AI_API_KEY";

/// Settings for creating a Google Generative AI provider.
#[derive(Debug, Clone, Default)]
pub struct GoogleGenerativeAIProviderSettings {
    /// Base URL for the API. Defaults to `https://generativelanguage.googleapis.com`.
    pub base_url: Option<String>,
    /// API key. If not provided, loaded from `GOOGLE_GENERATIVE_AI_API_KEY` env var.
    pub api_key: Option<String>,
    /// Additional headers to include in requests.
    pub headers: Option<HashMap<String, String>>,
    /// Provider name. Defaults to `"google.generative-ai"`.
    pub name: Option<String>,
}

/// Google Generative AI provider.
///
/// Factory for creating language, embedding, image, and video models
/// that connect to Google's Gemini API.
pub struct GoogleGenerativeAIProvider {
    provider_name: String,
    base_url: String,
    api_key: Option<String>,
    extra_headers: HashMap<String, String>,
}

impl GoogleGenerativeAIProvider {
    /// Create a new provider with the given settings.
    fn new(settings: GoogleGenerativeAIProviderSettings) -> Self {
        let base_url = settings
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        Self {
            provider_name: settings
                .name
                .unwrap_or_else(|| "google.generative-ai".to_string()),
            base_url: without_trailing_slash(&base_url).to_string(),
            api_key: settings.api_key,
            extra_headers: settings.headers.unwrap_or_default(),
        }
    }

    /// Build headers for API requests.
    fn build_headers(&self) -> Result<HashMap<String, String>, AISdkError> {
        let api_key = load_api_key(
            self.api_key.as_deref(),
            DEFAULT_API_KEY_ENV,
            "Google Generative AI",
        )
        .map_err(|e| AISdkError::new(e.to_string()))?;

        let mut headers = HashMap::new();
        headers.insert("x-goog-api-key".to_string(), api_key);
        headers.insert("content-type".to_string(), "application/json".to_string());

        // Merge extra headers
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        Ok(headers)
    }

    /// Create a language model for the given model ID.
    pub fn language_model_instance(
        &self,
        model_id: &str,
    ) -> Result<GoogleGenerativeAILanguageModel, AISdkError> {
        let headers = self.build_headers()?;
        let base_url = self.base_url.clone();
        let provider = self.provider_name.clone();

        let supported_urls_base = base_url.clone();
        let supported_urls_fn: Arc<dyn Fn() -> HashMap<String, Vec<Regex>> + Send + Sync> =
            Arc::new(move || {
                let escaped_base = regex::escape(&supported_urls_base);
                let mut patterns = Vec::new();
                // Google Generative Language "files" endpoint
                if let Ok(re) = Regex::new(&format!("^{escaped_base}/files/.*$")) {
                    patterns.push(re);
                }
                // YouTube URLs (public or unlisted videos)
                if let Ok(re) =
                    Regex::new(r"^https://(?:www\.)?youtube\.com/watch\?v=[\w-]+(?:&[\w=&.-]*)?$")
                {
                    patterns.push(re);
                }
                if let Ok(re) = Regex::new(r"^https://youtu\.be/[\w-]+(?:\?[\w=&.-]*)?$") {
                    patterns.push(re);
                }
                let mut map = HashMap::new();
                map.insert("*".to_string(), patterns);
                map
            });

        Ok(GoogleGenerativeAILanguageModel::new(
            model_id,
            GoogleGenerativeAILanguageModelConfig {
                provider,
                base_url,
                headers: Arc::new(move || headers.clone()),
                generate_id: Arc::new(|| vercel_ai_provider_utils::generate_id("google")),
                supported_urls: Some(supported_urls_fn),
                client: None,
            },
        ))
    }

    /// Create an embedding model for the given model ID.
    pub fn embedding_model_instance(
        &self,
        model_id: &str,
    ) -> Result<GoogleGenerativeAIEmbeddingModel, AISdkError> {
        let headers = self.build_headers()?;
        let base_url = self.base_url.clone();
        let provider = self.provider_name.clone();

        Ok(GoogleGenerativeAIEmbeddingModel::new(
            model_id,
            GoogleGenerativeAIEmbeddingModelConfig {
                provider,
                base_url,
                headers: Arc::new(move || headers.clone()),
                client: None,
            },
        ))
    }

    /// Create an image model for the given model ID.
    pub fn image_model_instance(
        &self,
        model_id: &str,
    ) -> Result<GoogleGenerativeAIImageModel, AISdkError> {
        let headers = self.build_headers()?;
        let base_url = self.base_url.clone();
        let provider = self.provider_name.clone();

        Ok(GoogleGenerativeAIImageModel::new(
            model_id,
            GoogleGenerativeAIImageSettings::default(),
            GoogleGenerativeAIImageModelConfig {
                provider,
                base_url,
                headers: Arc::new(move || headers.clone()),
                client: None,
            },
        ))
    }

    /// Create a video model for the given model ID.
    pub fn video_model_instance(
        &self,
        model_id: &str,
    ) -> Result<GoogleGenerativeAIVideoModel, AISdkError> {
        let headers = self.build_headers()?;
        let base_url = self.base_url.clone();
        let provider = self.provider_name.clone();

        Ok(GoogleGenerativeAIVideoModel::new(
            model_id,
            GoogleGenerativeAIVideoModelConfig {
                provider,
                base_url,
                headers: Arc::new(move || headers.clone()),
                client: None,
            },
        ))
    }
}

#[async_trait::async_trait]
impl ProviderV4 for GoogleGenerativeAIProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        let model = self.language_model_instance(model_id).map_err(|e| {
            NoSuchModelError::new(format!(
                "Model '{}' not available from provider '{}': {}",
                model_id, self.provider_name, e
            ))
        })?;
        Ok(Arc::new(model))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        let model = self.embedding_model_instance(model_id).map_err(|e| {
            NoSuchModelError::new(format!(
                "Model '{}' not available from provider '{}': {}",
                model_id, self.provider_name, e
            ))
        })?;
        Ok(Arc::new(model))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        let model = self.image_model_instance(model_id).map_err(|e| {
            NoSuchModelError::new(format!(
                "Model '{}' not available from provider '{}': {}",
                model_id, self.provider_name, e
            ))
        })?;
        Ok(Arc::new(model))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        let model = self.video_model_instance(model_id).map_err(|e| {
            NoSuchModelError::new(format!(
                "Model '{}' not available from provider '{}': {}",
                model_id, self.provider_name, e
            ))
        })?;
        Ok(Arc::new(model))
    }
}

/// Create a Google Generative AI provider with the given settings.
pub fn create_google_generative_ai(
    settings: GoogleGenerativeAIProviderSettings,
) -> GoogleGenerativeAIProvider {
    GoogleGenerativeAIProvider::new(settings)
}

/// Create a default Google Generative AI provider.
///
/// Uses the `GOOGLE_GENERATIVE_AI_API_KEY` environment variable for authentication.
pub fn google() -> GoogleGenerativeAIProvider {
    create_google_generative_ai(GoogleGenerativeAIProviderSettings::default())
}

#[cfg(test)]
#[path = "google_provider.test.rs"]
mod tests;
