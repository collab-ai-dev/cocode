//! Provider trait (V4).
//!
//! This module defines the `ProviderV4` trait for implementing AI providers
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::embedding_model::EmbeddingModelV4;
use crate::errors::NoSuchModelError;
use crate::files::FilesV4;
use crate::image_model::ImageModelV4;
use crate::language_model::LanguageModelV4;
use crate::reranking_model::RerankingModelV4;
use crate::speech_model::SpeechModelV4;
use crate::transcription_model::TranscriptionModelV4;
use crate::video_model::VideoModelV4;

/// The provider trait (V4).
///
/// This trait defines the interface for AI providers following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait ProviderV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get a language model by ID.
    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError>;

    /// Get an embedding model by ID.
    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError>;

    /// Get an image model by ID.
    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError>;

    /// Get a transcription model by ID.
    ///
    /// Default implementation returns `NoSuchModelError` since transcription
    /// is optional. Override this method if the provider supports transcription.
    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "transcriptionModel",
        ))
    }

    /// Get a speech model by ID.
    ///
    /// Default implementation returns `NoSuchModelError` since speech
    /// is optional. Override this method if the provider supports speech synthesis.
    fn speech_model(&self, model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "speechModel",
        ))
    }

    /// Get a reranking model by ID.
    ///
    /// Default implementation returns `NoSuchModelError` since reranking
    /// is optional. Override this method if the provider supports reranking.
    fn reranking_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn RerankingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "rerankingModel",
        ))
    }

    /// Get a video model by ID.
    ///
    /// Default implementation returns `NoSuchModelError` since video generation
    /// is optional. Override this method if the provider supports video generation.
    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "videoModel",
        ))
    }

    /// Get the files interface for uploading files to this provider.
    ///
    /// Returns `None` if the provider does not support file uploads.
    fn files(&self) -> Option<&dyn FilesV4> {
        None
    }
}

/// A simple provider that returns pre-configured models.
pub struct SimpleProvider {
    /// The provider name.
    name: String,
    /// Language models by ID.
    language_models: HashMap<String, Arc<dyn LanguageModelV4>>,
    /// Embedding models by ID.
    embedding_models: HashMap<String, Arc<dyn EmbeddingModelV4>>,
    /// Image models by ID.
    image_models: HashMap<String, Arc<dyn ImageModelV4>>,
    /// Transcription models by ID.
    transcription_models: HashMap<String, Arc<dyn TranscriptionModelV4>>,
    /// Speech models by ID.
    speech_models: HashMap<String, Arc<dyn SpeechModelV4>>,
    /// Reranking models by ID.
    reranking_models: HashMap<String, Arc<dyn RerankingModelV4>>,
    /// Video models by ID.
    video_models: HashMap<String, Arc<dyn VideoModelV4>>,
}

impl SimpleProvider {
    /// Create a new simple provider.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            language_models: HashMap::new(),
            embedding_models: HashMap::new(),
            image_models: HashMap::new(),
            transcription_models: HashMap::new(),
            speech_models: HashMap::new(),
            reranking_models: HashMap::new(),
            video_models: HashMap::new(),
        }
    }

    /// Add a language model.
    pub fn with_language_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn LanguageModelV4>,
    ) -> Self {
        self.language_models.insert(model_id.into(), model);
        self
    }

    /// Add an embedding model.
    pub fn with_embedding_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn EmbeddingModelV4>,
    ) -> Self {
        self.embedding_models.insert(model_id.into(), model);
        self
    }

    /// Add an image model.
    pub fn with_image_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn ImageModelV4>,
    ) -> Self {
        self.image_models.insert(model_id.into(), model);
        self
    }

    /// Add a transcription model.
    pub fn with_transcription_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn TranscriptionModelV4>,
    ) -> Self {
        self.transcription_models.insert(model_id.into(), model);
        self
    }

    /// Add a speech model.
    pub fn with_speech_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn SpeechModelV4>,
    ) -> Self {
        self.speech_models.insert(model_id.into(), model);
        self
    }

    /// Add a reranking model.
    pub fn with_reranking_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn RerankingModelV4>,
    ) -> Self {
        self.reranking_models.insert(model_id.into(), model);
        self
    }

    /// Add a video model.
    pub fn with_video_model(
        mut self,
        model_id: impl Into<String>,
        model: Arc<dyn VideoModelV4>,
    ) -> Self {
        self.video_models.insert(model_id.into(), model);
        self
    }
}

#[async_trait]
impl ProviderV4 for SimpleProvider {
    fn provider(&self) -> &str {
        &self.name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        self.language_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        self.embedding_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn image_model(&self, model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        self.image_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        self.transcription_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn speech_model(&self, model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        self.speech_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn reranking_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn RerankingModelV4>, NoSuchModelError> {
        self.reranking_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        self.video_models
            .get(model_id)
            .cloned()
            .ok_or_else(|| NoSuchModelError::for_model(model_id))
    }
}

/// Trait for providers that can be configured.
pub trait ConfigurableProvider: ProviderV4 {
    /// The configuration type for this provider.
    type Config;

    /// Create a new provider with the given configuration.
    fn with_config(config: Self::Config) -> Self;
}

/// Trait for providers that can be created from environment variables.
#[async_trait]
pub trait FromEnvProvider: ProviderV4 + Sized {
    /// Create a new provider from environment variables.
    fn from_env() -> Result<Self, crate::errors::LoadAPIKeyError>;
}

#[cfg(test)]
#[path = "provider_v4.test.rs"]
mod tests;
