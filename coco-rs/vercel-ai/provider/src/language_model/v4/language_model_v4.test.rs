use super::*;

/// Mock implementation for testing
struct MockLanguageModel {
    provider: String,
    model_id: String,
}

#[async_trait::async_trait]
impl LanguageModelV4 for MockLanguageModel {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, crate::errors::AISdkError> {
        Ok(LanguageModelV4GenerateResult::text(
            "Mock response",
            crate::language_model::Usage::new(10, 5),
        ))
    }

    async fn do_stream(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, crate::errors::AISdkError> {
        unimplemented!("Mock stream not implemented")
    }
}

#[test]
fn test_specification_version() {
    let model = MockLanguageModel {
        provider: "test".to_string(),
        model_id: "test-model".to_string(),
    };
    assert_eq!(model.specification_version(), "v4");
}

#[test]
fn test_provider_and_model_id() {
    let model = MockLanguageModel {
        provider: "openai".to_string(),
        model_id: "gpt-4".to_string(),
    };
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), "gpt-4");
}
