//! Provider registry for managing multiple providers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::RerankingModelV4;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::VideoModelV4;

use crate::middleware::LanguageModelV4Middleware;
use crate::middleware::wrap_language_model;
use crate::registry::NoSuchProviderError;

/// Options for creating a provider registry.
#[derive(Default)]
pub struct ProviderRegistryOptions {
    /// The separator used between provider ID and model ID. Defaults to ":".
    pub separator: String,
    /// Middleware to apply to all language models.
    pub language_model_middleware: Vec<Arc<dyn LanguageModelV4Middleware>>,
}

/// A registry for managing multiple providers.
///
/// The registry allows you to access models using a combined identifier
/// in the format `providerId{separator}modelId`.
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn ProviderV4>>>,
    separator: String,
    language_model_middleware: Vec<Arc<dyn LanguageModelV4Middleware>>,
}

impl ProviderRegistry {
    /// Create a new provider registry with the given options.
    pub fn new(options: ProviderRegistryOptions) -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            separator: if options.separator.is_empty() {
                ":".to_string()
            } else {
                options.separator
            },
            language_model_middleware: options.language_model_middleware,
        }
    }

    /// Register a provider with the given ID.
    #[allow(clippy::expect_used)]
    pub fn register_provider(&self, id: impl Into<String>, provider: Arc<dyn ProviderV4>) {
        let mut providers = self.providers.write().expect("lock poisoned");
        providers.insert(id.into(), provider);
    }

    /// Get a language model by its combined identifier.
    ///
    /// The identifier should be in the format `providerId{separator}modelId`.
    pub fn language_model(&self, id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "languageModel")?;

        let provider = self.get_provider(&provider_id)?;

        let model = provider.language_model(&model_id)?;

        if !self.language_model_middleware.is_empty() {
            Ok(wrap_language_model(
                model,
                self.language_model_middleware.clone(),
            ))
        } else {
            Ok(model)
        }
    }

    /// Get an embedding model by its combined identifier.
    pub fn embedding_model(&self, id: &str) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "embeddingModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.embedding_model(&model_id)
    }

    /// Get an image model by its combined identifier.
    pub fn image_model(&self, id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "imageModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.image_model(&model_id)
    }

    /// Get a transcription model by its combined identifier.
    pub fn transcription_model(
        &self,
        id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "transcriptionModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.transcription_model(&model_id)
    }

    /// Get a speech model by its combined identifier.
    pub fn speech_model(&self, id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "speechModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.speech_model(&model_id)
    }

    /// Get a reranking model by its combined identifier.
    pub fn reranking_model(&self, id: &str) -> Result<Arc<dyn RerankingModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "rerankingModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.reranking_model(&model_id)
    }

    /// Get a video model by its combined identifier.
    pub fn video_model(&self, id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        let (provider_id, model_id) = self.split_id(id, "videoModel")?;

        let provider = self.get_provider(&provider_id)?;

        provider.video_model(&model_id)
    }

    #[allow(clippy::expect_used)]
    fn get_provider(&self, id: &str) -> Result<Arc<dyn ProviderV4>, NoSuchModelError> {
        let providers = self.providers.read().expect("lock poisoned");

        if let Some(provider) = providers.get(id) {
            return Ok(provider.clone());
        }

        let available: Vec<String> = providers.keys().cloned().collect();
        drop(providers);

        Err(NoSuchProviderError::new(id, available).into())
    }

    fn split_id(&self, id: &str, model_type: &str) -> Result<(String, String), NoSuchModelError> {
        let index = id.find(&self.separator);

        match index {
            Some(idx) => {
                let provider_id = &id[..idx];
                let model_id = &id[idx + self.separator.len()..];
                Ok((provider_id.to_string(), model_id.to_string()))
            }
            None => Err(NoSuchModelError::new(format!(
                "Invalid {} id for registry: {} (must be in the format \"providerId{}modelId\")",
                model_type, id, self.separator
            ))),
        }
    }
}

/// Create a provider registry with the given providers and options.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{create_provider_registry, ProviderRegistryOptions};
/// use std::sync::Arc;
///
/// let registry = create_provider_registry(
///     vec![("anthropic".to_string(), anthropic_provider)].into_iter().collect(),
///     ProviderRegistryOptions::default(),
/// );
///
/// let model = registry.language_model("anthropic:claude-3-sonnet");
/// ```
pub fn create_provider_registry(
    providers: HashMap<String, Arc<dyn ProviderV4>>,
    options: ProviderRegistryOptions,
) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new(options));

    for (id, provider) in providers {
        registry.register_provider(id, provider);
    }

    registry
}

#[cfg(test)]
#[path = "provider_registry.test.rs"]
mod tests;
