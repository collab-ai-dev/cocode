//! Mock provider for testing.

use std::collections::HashMap;
use std::sync::Arc;

use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::VideoModelV4;

/// A configurable mock provider for testing.
pub struct MockProvider {
    language_models: HashMap<String, Arc<dyn LanguageModelV4>>,
    embedding_models: HashMap<String, Arc<dyn EmbeddingModelV4>>,
    image_models: HashMap<String, Arc<dyn ImageModelV4>>,
    video_models: HashMap<String, Arc<dyn VideoModelV4>>,
}

impl MockProvider {
    /// Create a new empty mock provider.
    pub fn new() -> Self {
        Self {
            language_models: HashMap::new(),
            embedding_models: HashMap::new(),
            image_models: HashMap::new(),
            video_models: HashMap::new(),
        }
    }

    /// Add a language model.
    pub fn with_language_model(
        mut self,
        id: impl Into<String>,
        model: Arc<dyn LanguageModelV4>,
    ) -> Self {
        self.language_models.insert(id.into(), model);
        self
    }

    /// Add an embedding model.
    pub fn with_embedding_model(
        mut self,
        id: impl Into<String>,
        model: Arc<dyn EmbeddingModelV4>,
    ) -> Self {
        self.embedding_models.insert(id.into(), model);
        self
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderV4 for MockProvider {
    fn provider(&self) -> &str {
        "mock"
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        self.language_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::new(model_id))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        self.embedding_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::new(model_id))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        self.image_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::new(model_id))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        self.video_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::new(model_id))
    }
}
