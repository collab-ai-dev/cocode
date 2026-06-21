//! Code Assist provider factory (`ProviderV4`).

use std::collections::HashMap;
use std::sync::Arc;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::auth::CodeAssistCredsSupplier;
use crate::language_model::GoogleCodeAssistLanguageModel;

/// Settings for a Gemini Code Assist provider. Code Assist is OAuth-only, so
/// `creds` (a per-request Bearer supplier) is required.
#[derive(Clone)]
pub struct GoogleCodeAssistProviderSettings {
    /// Base URL. Defaults to [`crate::CODE_ASSIST_BASE_URL`].
    pub base_url: Option<String>,
    /// Per-request OAuth credential supplier.
    pub creds: CodeAssistCredsSupplier,
    /// Extra headers merged onto every request.
    pub headers: Option<HashMap<String, String>>,
    /// Provider name. Defaults to `"google.code-assist"`.
    pub name: Option<String>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// Gemini Code Assist provider.
pub struct GoogleCodeAssistProvider {
    provider_name: String,
    base_url: String,
    creds: CodeAssistCredsSupplier,
    extra_headers: HashMap<String, String>,
    client: Arc<reqwest::Client>,
}

impl GoogleCodeAssistProvider {
    fn new(settings: GoogleCodeAssistProviderSettings) -> Self {
        let base_url = settings
            .base_url
            .unwrap_or_else(|| crate::CODE_ASSIST_BASE_URL.to_string());
        Self {
            provider_name: settings
                .name
                .unwrap_or_else(|| "google.code-assist".to_string()),
            base_url: without_trailing_slash(&base_url).to_string(),
            creds: settings.creds,
            extra_headers: settings.headers.unwrap_or_default(),
            client: settings
                .client
                .unwrap_or_else(|| Arc::new(reqwest::Client::new())),
        }
    }

    /// Build a concrete Code Assist language model for `model_id`.
    pub fn language_model_instance(&self, model_id: &str) -> GoogleCodeAssistLanguageModel {
        GoogleCodeAssistLanguageModel::new(
            model_id,
            self.provider_name.clone(),
            self.base_url.clone(),
            self.creds.clone(),
            self.extra_headers.clone(),
            self.client.clone(),
        )
    }
}

#[async_trait::async_trait]
impl ProviderV4 for GoogleCodeAssistProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.language_model_instance(model_id)))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "embeddingModel",
        ))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "imageModel",
        ))
    }
}

/// Create a Gemini Code Assist provider from settings.
pub fn create_google_code_assist(
    settings: GoogleCodeAssistProviderSettings,
) -> GoogleCodeAssistProvider {
    GoogleCodeAssistProvider::new(settings)
}
