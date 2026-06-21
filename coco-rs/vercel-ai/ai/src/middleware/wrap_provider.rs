//! Wrap a provider with middleware.

use std::sync::Arc;

use vercel_ai_provider::NoSuchModelError;
use vercel_ai_provider::ProviderV4;

use crate::middleware::ImageMiddleware;
use crate::middleware::LanguageModelV4Middleware;
use crate::middleware::wrap_image_model;
use crate::middleware::wrap_language_model;

/// Wrap a provider with middleware for all its models.
///
/// This applies the given middleware to all language and image models
/// retrieved from the provider.
pub fn wrap_provider(
    provider: Arc<dyn ProviderV4>,
    language_model_middleware: Vec<Arc<dyn LanguageModelV4Middleware>>,
    image_model_middleware: Option<Arc<dyn ImageMiddleware>>,
) -> Arc<dyn ProviderV4> {
    Arc::new(ProviderWrapper {
        provider,
        language_model_middleware,
        image_model_middleware,
    })
}

/// Internal wrapper for provider middleware.
struct ProviderWrapper {
    provider: Arc<dyn ProviderV4>,
    language_model_middleware: Vec<Arc<dyn LanguageModelV4Middleware>>,
    image_model_middleware: Option<Arc<dyn ImageMiddleware>>,
}

#[async_trait::async_trait]
impl ProviderV4 for ProviderWrapper {
    fn provider(&self) -> &str {
        self.provider.provider()
    }

    fn language_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::LanguageModelV4>, NoSuchModelError> {
        let model = self.provider.language_model(model_id)?;

        if !self.language_model_middleware.is_empty() {
            Ok(wrap_language_model(
                model,
                self.language_model_middleware.clone(),
            ))
        } else {
            Ok(model)
        }
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::EmbeddingModelV4>, NoSuchModelError> {
        self.provider.embedding_model(model_id)
    }

    fn image_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::ImageModelV4>, NoSuchModelError> {
        let model = self.provider.image_model(model_id)?;

        if let Some(ref middleware) = self.image_model_middleware {
            Ok(wrap_image_model(model, middleware.clone()))
        } else {
            Ok(model)
        }
    }

    fn transcription_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::TranscriptionModelV4>, NoSuchModelError> {
        self.provider.transcription_model(model_id)
    }

    fn speech_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::SpeechModelV4>, NoSuchModelError> {
        self.provider.speech_model(model_id)
    }

    fn reranking_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::RerankingModelV4>, NoSuchModelError> {
        self.provider.reranking_model(model_id)
    }

    fn video_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::VideoModelV4>, NoSuchModelError> {
        self.provider.video_model(model_id)
    }
}

#[cfg(test)]
#[path = "wrap_provider.test.rs"]
mod tests;
