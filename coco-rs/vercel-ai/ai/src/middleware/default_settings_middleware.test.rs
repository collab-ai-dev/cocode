use super::*;
use std::sync::Arc;
use vercel_ai_provider::language_model_middleware::CallType;
use vercel_ai_provider::language_model_middleware::TransformParamsOptions;

#[tokio::test]
async fn test_default_settings_applied() {
    let settings = DefaultSettings {
        temperature: Some(0.7),
        max_output_tokens: Some(1000),
        ..Default::default()
    };

    let middleware = default_settings_middleware(settings);

    let params = LanguageModelV4CallOptions::default();
    let result = middleware
        .transform_params(TransformParamsOptions {
            call_type: CallType::Generate,
            params,
            model: Arc::new(MockModel),
        })
        .await
        .unwrap();

    assert_eq!(result.temperature, Some(0.7));
    assert_eq!(result.max_output_tokens, Some(1000));
}

#[tokio::test]
async fn test_call_params_override_defaults() {
    let settings = DefaultSettings {
        temperature: Some(0.7),
        ..Default::default()
    };

    let middleware = default_settings_middleware(settings);

    let params = LanguageModelV4CallOptions {
        temperature: Some(0.9),
        ..Default::default()
    };

    let result = middleware
        .transform_params(TransformParamsOptions {
            call_type: CallType::Generate,
            params,
            model: Arc::new(MockModel),
        })
        .await
        .unwrap();

    // Call params should override defaults
    assert_eq!(result.temperature, Some(0.9));
}

struct MockModel;

#[async_trait::async_trait]
impl vercel_ai_provider::LanguageModelV4 for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock"
    }
    async fn do_generate(
        &self,
        _: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<vercel_ai_provider::LanguageModelV4GenerateResult, AISdkError> {
        unimplemented!()
    }
    async fn do_stream(
        &self,
        _: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<vercel_ai_provider::LanguageModelV4StreamResult, AISdkError> {
        unimplemented!()
    }
}
