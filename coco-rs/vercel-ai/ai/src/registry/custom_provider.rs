//! Custom provider creation.

use std::collections::HashMap;
use std::sync::Arc;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::RerankingModelV4;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::VideoModelV4;

/// Options for creating a custom provider.
#[derive(Default)]
pub struct CustomProviderOptions {
    /// Language models to include in the provider.
    pub language_models: HashMap<String, Arc<dyn LanguageModelV4>>,
    /// Embedding models to include in the provider.
    pub embedding_models: HashMap<String, Arc<dyn EmbeddingModelV4>>,
    /// Image models to include in the provider.
    pub image_models: HashMap<String, Arc<dyn ImageModelV4>>,
    /// Transcription models to include in the provider.
    pub transcription_models: HashMap<String, Arc<dyn TranscriptionModelV4>>,
    /// Speech models to include in the provider.
    pub speech_models: HashMap<String, Arc<dyn SpeechModelV4>>,
    /// Reranking models to include in the provider.
    pub reranking_models: HashMap<String, Arc<dyn RerankingModelV4>>,
    /// Video models to include in the provider.
    pub video_models: HashMap<String, Arc<dyn VideoModelV4>>,
    /// Fallback provider to use when a model is not found.
    pub fallback_provider: Option<Arc<dyn ProviderV4>>,
}

/// Creates a custom provider with specified models and an optional fallback provider.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{custom_provider, CustomProviderOptions};
/// use std::sync::Arc;
///
/// let provider = custom_provider(CustomProviderOptions {
///     language_models: vec![("gpt-4".to_string(), my_model)].into_iter().collect(),
///     ..Default::default()
/// });
///
/// let model = provider.language_model("gpt-4");
/// ```
pub fn custom_provider(options: CustomProviderOptions) -> Arc<dyn ProviderV4> {
    Arc::new(CustomProvider {
        language_models: options.language_models,
        embedding_models: options.embedding_models,
        image_models: options.image_models,
        transcription_models: options.transcription_models,
        speech_models: options.speech_models,
        reranking_models: options.reranking_models,
        video_models: options.video_models,
        fallback_provider: options.fallback_provider,
    })
}

/// Internal implementation of a custom provider.
struct CustomProvider {
    language_models: HashMap<String, Arc<dyn LanguageModelV4>>,
    embedding_models: HashMap<String, Arc<dyn EmbeddingModelV4>>,
    image_models: HashMap<String, Arc<dyn ImageModelV4>>,
    transcription_models: HashMap<String, Arc<dyn TranscriptionModelV4>>,
    speech_models: HashMap<String, Arc<dyn SpeechModelV4>>,
    reranking_models: HashMap<String, Arc<dyn RerankingModelV4>>,
    video_models: HashMap<String, Arc<dyn VideoModelV4>>,
    fallback_provider: Option<Arc<dyn ProviderV4>>,
}

#[async_trait::async_trait]
impl ProviderV4 for CustomProvider {
    fn provider(&self) -> &str {
        "custom"
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        if let Some(model) = self.language_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.language_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        if let Some(model) = self.embedding_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.embedding_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        if let Some(model) = self.image_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.image_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        if let Some(model) = self.transcription_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.transcription_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn speech_model(&self, model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        if let Some(model) = self.speech_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.speech_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn reranking_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn RerankingModelV4>, NoSuchModelError> {
        if let Some(model) = self.reranking_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.reranking_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        if let Some(model) = self.video_models.get(model_id) {
            return Ok(model.clone());
        }

        if let Some(ref fallback) = self.fallback_provider {
            return fallback.video_model(model_id);
        }

        Err(NoSuchModelError::for_model(model_id))
    }
}

#[cfg(test)]
#[path = "custom_provider.test.rs"]
mod tests;
