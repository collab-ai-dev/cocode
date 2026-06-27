//! Tests for language model middleware.

use super::*;

/// A simple test model that returns fixed responses.
struct TestModel {
    provider_name: &'static str,
    model_id: &'static str,
}

#[async_trait::async_trait]
impl LanguageModelV4 for TestModel {
    fn provider(&self) -> &str {
        self.provider_name
    }

    fn model_id(&self) -> &str {
        self.model_id
    }

    async fn do_generate(
        &self,
        _params: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        Ok(LanguageModelV4GenerateResult::text(
            "test response",
            crate::language_model::Usage::empty(),
        ))
    }

    async fn do_stream(
        &self,
        _params: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        // Return an error for streaming in test
        Err(AISdkError::new("streaming not supported in test"))
    }
}

/// A middleware that overrides provider and model ID.
struct OverrideMiddleware {
    provider: String,
    model_id: String,
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for OverrideMiddleware {
    fn override_provider(&self, _options: &MiddlewareOptions) -> Option<String> {
        Some(self.provider.clone())
    }

    fn override_model_id(&self, _options: &MiddlewareOptions) -> Option<String> {
        Some(self.model_id.clone())
    }
}

/// A middleware that transforms params.
struct TransformMiddleware;

#[async_trait::async_trait]
impl LanguageModelV4Middleware for TransformMiddleware {
    async fn transform_params(
        &self,
        mut options: TransformParamsOptions,
    ) -> Result<LanguageModelV4CallOptions, AISdkError> {
        // Add a custom header to all calls
        let mut headers = options.params.headers.take().unwrap_or_default();
        headers.insert("x-middleware".to_string(), "transformed".to_string());
        options.params.headers = Some(headers);
        Ok(options.params)
    }
}

#[test]
fn test_middleware_wrapper_provider_override() {
    let model = Arc::new(TestModel {
        provider_name: "original-provider",
        model_id: "original-model",
    });
    let middleware = Arc::new(OverrideMiddleware {
        provider: "new-provider".to_string(),
        model_id: "new-model".to_string(),
    });
    let wrapped = MiddlewareWrapper::new(model, middleware);

    assert_eq!(wrapped.provider(), "new-provider");
    assert_eq!(wrapped.model_id(), "new-model");
}

#[test]
fn test_middleware_chain() {
    let model = Arc::new(TestModel {
        provider_name: "test-provider",
        model_id: "test-model",
    });

    let chain = MiddlewareChain::new()
        .add(Arc::new(OverrideMiddleware {
            provider: "first-provider".to_string(),
            model_id: "first-model".to_string(),
        }))
        .add(Arc::new(OverrideMiddleware {
            provider: "second-provider".to_string(),
            model_id: "second-model".to_string(),
        }));

    let wrapped = chain.apply(model);

    // The last middleware in the chain should win
    assert_eq!(wrapped.provider(), "second-provider");
    assert_eq!(wrapped.model_id(), "second-model");
}

#[test]
fn test_middleware_no_override() {
    let model = Arc::new(TestModel {
        provider_name: "original-provider",
        model_id: "original-model",
    });

    // Empty middleware that doesn't override anything
    struct NoOpMiddleware;
    #[async_trait::async_trait]
    impl LanguageModelV4Middleware for NoOpMiddleware {}

    let middleware = Arc::new(NoOpMiddleware);
    let wrapped = MiddlewareWrapper::new(model, middleware);

    assert_eq!(wrapped.provider(), "original-provider");
    assert_eq!(wrapped.model_id(), "original-model");
}

#[tokio::test]
async fn test_middleware_transform_params() {
    let model = Arc::new(TestModel {
        provider_name: "test-provider",
        model_id: "test-model",
    });

    let middleware = Arc::new(TransformMiddleware);
    let wrapped = Arc::new(MiddlewareWrapper::new(model, middleware));

    let params = LanguageModelV4CallOptions::new(vec![]);
    let result = wrapped.do_generate(&params, None).await;

    assert!(result.is_ok());
}

#[test]
fn test_call_type() {
    assert_eq!(CallType::Generate, CallType::Generate);
    assert_ne!(CallType::Generate, CallType::Stream);
}
