use super::*;
use std::sync::Arc;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamResult;

// Mock language model for testing
struct MockLanguageModel {
    id: String,
}

#[async_trait::async_trait]
impl LanguageModelV4 for MockLanguageModel {
    fn provider(&self) -> &str {
        "mock"
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
fn test_custom_provider_with_language_model() {
    let model: Arc<dyn LanguageModelV4> = Arc::new(MockLanguageModel {
        id: "test-model".to_string(),
    });

    let provider = custom_provider(CustomProviderOptions {
        language_models: vec![("test-model".to_string(), model)]
            .into_iter()
            .collect(),
        ..Default::default()
    });

    let retrieved = provider.language_model("test-model").unwrap();
    assert_eq!(retrieved.model_id(), "test-model");
}

#[test]
fn test_custom_provider_missing_model() {
    let provider = custom_provider(CustomProviderOptions::default());
    assert!(provider.language_model("non-existent").is_err());
}
