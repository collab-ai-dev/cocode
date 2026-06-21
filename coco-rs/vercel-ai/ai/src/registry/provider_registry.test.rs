use super::*;
use std::sync::Arc;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::RerankingModelV4;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::VideoModelV4;

// Mock provider for testing
struct MockProvider {
    name: String,
}

#[async_trait::async_trait]
impl ProviderV4 for MockProvider {
    fn provider(&self) -> &str {
        &self.name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(MockLanguageModel {
            provider: self.name.clone(),
            id: model_id.to_string(),
        }))
    }

    fn embedding_model(
        &self,
        _model_id: &str,
    ) -> Result<Arc<dyn EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(_model_id))
    }

    fn image_model(&self, _model_id: &str) -> Result<Arc<dyn ImageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(_model_id))
    }

    fn transcription_model(
        &self,
        _model_id: &str,
    ) -> Result<Arc<dyn TranscriptionModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(_model_id))
    }

    fn speech_model(&self, _model_id: &str) -> Result<Arc<dyn SpeechModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(_model_id))
    }

    fn reranking_model(
        &self,
        _model_id: &str,
    ) -> Result<Arc<dyn RerankingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(_model_id))
    }

    fn video_model(&self, model_id: &str) -> Result<Arc<dyn VideoModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model(model_id))
    }
}

struct MockLanguageModel {
    provider: String,
    id: String,
}

#[async_trait::async_trait]
impl LanguageModelV4 for MockLanguageModel {
    fn provider(&self) -> &str {
        &self.provider
    }
    fn model_id(&self) -> &str {
        &self.id
    }

    async fn do_generate(
        &self,
        _params: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, vercel_ai_provider::AISdkError> {
        unimplemented!()
    }

    async fn do_stream(
        &self,
        _params: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, vercel_ai_provider::AISdkError> {
        unimplemented!()
    }
}

#[test]
fn test_registry_language_model() {
    let provider = Arc::new(MockProvider {
        name: "test".to_string(),
    });
    let mut providers = HashMap::new();
    providers.insert("test".to_string(), provider as Arc<dyn ProviderV4>);

    let registry = create_provider_registry(providers, ProviderRegistryOptions::default());

    let model = registry.language_model("test:model-123").unwrap();
    assert_eq!(model.provider(), "test");
    assert_eq!(model.model_id(), "model-123");
}

#[test]
fn test_registry_invalid_id_format() {
    let registry = ProviderRegistry::new(ProviderRegistryOptions::default());
    assert!(registry.language_model("no-separator").is_err());
}

#[test]
fn test_registry_missing_provider() {
    let registry = ProviderRegistry::new(ProviderRegistryOptions::default());
    assert!(registry.language_model("missing:model").is_err());
}
