//! ByteDance Seedance provider factory.
//!
//! Creates instances of ByteDance video models.

use std::collections::HashMap;
use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::VideoModelV4;

use vercel_ai_provider_utils::load_api_key;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::bytedance_config::ByteDanceVideoModelConfig;
use crate::bytedance_video_model::ByteDanceVideoModel;

/// Default base URL for the ByteDance ModelArk API.
const DEFAULT_BASE_URL: &str = "https://ark.ap-southeast.bytepluses.com/api/v3";

/// Default environment variable for the ByteDance API key.
const DEFAULT_API_KEY_ENV: &str = "ARK_API_KEY";

/// Settings for creating a ByteDance provider.
#[derive(Default)]
pub struct ByteDanceProviderSettings {
    /// Base URL for the API. Defaults to `https://ark.ap-southeast.bytepluses.com/api/v3`.
    pub base_url: Option<String>,
    /// API key. If not provided, loaded from `ARK_API_KEY` env var.
    pub api_key: Option<String>,
    /// Additional headers to include in requests.
    pub headers: Option<HashMap<String, String>>,
    /// Provider name. Defaults to `"bytedance"`.
    pub name: Option<String>,
    /// Shared HTTP client.
    pub client: Option<Arc<reqwest::Client>>,
}

/// ByteDance provider.
///
/// Factory for creating video models that connect to the ByteDance ModelArk API
/// for Seedance video generation.
pub struct ByteDanceProvider {
    provider_name: String,
    base_url: String,
    api_key: Option<String>,
    extra_headers: HashMap<String, String>,
    client: Option<Arc<reqwest::Client>>,
}

impl ByteDanceProvider {
    /// Create a new provider with the given settings.
    fn new(settings: ByteDanceProviderSettings) -> Self {
        let base_url = settings
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        Self {
            provider_name: settings.name.unwrap_or_else(|| "bytedance".to_string()),
            base_url: without_trailing_slash(&base_url).to_string(),
            api_key: settings.api_key,
            extra_headers: settings.headers.unwrap_or_default(),
            client: settings.client,
        }
    }

    /// Build headers for API requests (used in tests to verify header logic).
    #[cfg(test)]
    fn build_headers(&self) -> Result<HashMap<String, String>, AISdkError> {
        let api_key = load_api_key(self.api_key.as_deref(), DEFAULT_API_KEY_ENV, "ByteDance")
            .map_err(|e| AISdkError::new(e.to_string()))?;

        let mut headers = HashMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {api_key}"));
        headers.insert("content-type".to_string(), "application/json".to_string());

        // Merge extra headers
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        Ok(headers)
    }

    /// Create a video model for the given model ID.
    pub fn video_model_instance(&self, model_id: &str) -> Result<ByteDanceVideoModel, AISdkError> {
        let base_url = self.base_url.clone();
        let provider = self.provider_name.clone();

        // Capture values for lazy header evaluation (per-request)
        let api_key = self.api_key.clone();
        let extra_headers = self.extra_headers.clone();

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let key = load_api_key(api_key.as_deref(), DEFAULT_API_KEY_ENV, "ByteDance")
                .unwrap_or_default();

            let mut h = HashMap::new();
            if !key.is_empty() {
                h.insert("authorization".to_string(), format!("Bearer {key}"));
            }
            h.insert("content-type".to_string(), "application/json".to_string());

            for (k, v) in &extra_headers {
                h.insert(k.clone(), v.clone());
            }
            h
        });

        Ok(ByteDanceVideoModel::new(
            model_id,
            ByteDanceVideoModelConfig {
                provider,
                base_url,
                headers,
                client: self.client.clone(),
                poll_interval: None,
                poll_timeout: None,
            },
        ))
    }
}

#[async_trait::async_trait]
impl ProviderV4 for ByteDanceProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::new(format!(
            "ByteDance provider does not support language models (requested '{model_id}')"
        )))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::new(format!(
            "ByteDance provider does not support embedding models (requested '{model_id}')"
        )))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::new(format!(
            "ByteDance provider does not support image models (requested '{model_id}')"
        )))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        let model = self.video_model_instance(model_id).map_err(|e| {
            NoSuchModelError::new(format!(
                "Model '{model_id}' not available from provider '{}': {e}",
                self.provider_name
            ))
        })?;
        Ok(Arc::new(model))
    }
}

/// Create a ByteDance provider with the given settings.
pub fn create_bytedance(settings: ByteDanceProviderSettings) -> ByteDanceProvider {
    ByteDanceProvider::new(settings)
}

/// Create a default ByteDance provider.
///
/// Uses the `ARK_API_KEY` environment variable for authentication.
pub fn bytedance() -> ByteDanceProvider {
    create_bytedance(ByteDanceProviderSettings::default())
}

#[cfg(test)]
#[path = "bytedance_provider.test.rs"]
mod tests;
