//! Wrap a language model with middleware.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::LanguageModelV4StreamResult;

/// Wrap a language model with middleware.
///
/// When multiple middlewares are provided, they are applied in reverse order
/// (the first middleware in the list wraps the result of the second, etc.).
///
/// # Example
///
/// ```ignore
/// use vercel_ai::middleware::{wrap_language_model, default_settings_middleware};
/// use std::sync::Arc;
///
/// let wrapped = wrap_language_model(
///     model,
///     vec![Arc::new(default_settings_middleware(settings))],
/// );
/// ```
pub fn wrap_language_model(
    model: Arc<dyn LanguageModelV4>,
    middleware: Vec<Arc<dyn LanguageModelV4Middleware>>,
) -> Arc<dyn LanguageModelV4> {
    // Apply middleware in reverse order
    middleware.into_iter().rev().fold(model, |wrapped, m| {
        Arc::new(MiddlewareWrapper::new(wrapped, m))
    })
}

/// Internal wrapper that applies a single middleware.
struct MiddlewareWrapper {
    inner: Arc<dyn LanguageModelV4>,
    middleware: Arc<dyn LanguageModelV4Middleware>,
    provider_override: Option<String>,
    model_id_override: Option<String>,
}

impl MiddlewareWrapper {
    fn new(
        inner: Arc<dyn LanguageModelV4>,
        middleware: Arc<dyn LanguageModelV4Middleware>,
    ) -> Self {
        use vercel_ai_provider::language_model_middleware::MiddlewareOptions;

        let options = MiddlewareOptions {
            model: inner.clone(),
        };
        let provider_override = middleware.override_provider(&options);
        let model_id_override = middleware.override_model_id(&options);

        Self {
            inner,
            middleware,
            provider_override,
            model_id_override,
        }
    }
}

#[async_trait::async_trait]
impl LanguageModelV4 for MiddlewareWrapper {
    fn provider(&self) -> &str {
        match &self.provider_override {
            Some(s) => s.as_str(),
            None => self.inner.provider(),
        }
    }

    fn model_id(&self) -> &str {
        match &self.model_id_override {
            Some(s) => s.as_str(),
            None => self.inner.model_id(),
        }
    }

    fn supported_urls(&self) -> std::collections::HashMap<String, Vec<regex::Regex>> {
        use vercel_ai_provider::language_model_middleware::MiddlewareOptions;
        let options = MiddlewareOptions {
            model: self.inner.clone(),
        };
        self.middleware
            .override_supported_urls(&options)
            .unwrap_or_else(|| self.inner.supported_urls())
    }

    async fn do_generate(
        &self,
        params: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        use vercel_ai_provider::language_model_middleware::CallType;
        use vercel_ai_provider::language_model_middleware::TransformParamsOptions;
        use vercel_ai_provider::language_model_middleware::WrapGenerateOptions;

        // Transform params — middleware may produce a fresh modified copy.
        let transform_options = TransformParamsOptions {
            call_type: CallType::Generate,
            params: params.clone(),
            model: self.inner.clone(),
        };
        let transformed_params = self.middleware.transform_params(transform_options).await?;

        // Wrap generate. Closure receives owned params + abort, forwards
        // `&params + abort` to the leaf provider.
        let inner = self.inner.clone();
        let wrap_options = WrapGenerateOptions {
            params: transformed_params,
            abort_signal,
            model: inner.clone(),
            do_generate: Box::new(move |p, abort| {
                let inner = inner.clone();
                Box::pin(async move { inner.do_generate(&p, abort).await })
            }),
        };
        self.middleware.wrap_generate(wrap_options).await
    }

    async fn do_stream(
        &self,
        params: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        use vercel_ai_provider::language_model_middleware::CallType;
        use vercel_ai_provider::language_model_middleware::TransformParamsOptions;
        use vercel_ai_provider::language_model_middleware::WrapStreamOptions;

        // Transform params — middleware may produce a fresh modified copy.
        let transform_options = TransformParamsOptions {
            call_type: CallType::Stream,
            params: params.clone(),
            model: self.inner.clone(),
        };
        let transformed_params = self.middleware.transform_params(transform_options).await?;

        // Wrap stream. Same pattern as `do_generate`.
        let inner = self.inner.clone();
        let wrap_options = WrapStreamOptions {
            params: transformed_params,
            abort_signal,
            model: inner.clone(),
            do_stream: Box::new(move |p, abort| {
                let inner = inner.clone();
                Box::pin(async move { inner.do_stream(&p, abort).await })
            }),
        };
        self.middleware.wrap_stream(wrap_options).await
    }
}

#[cfg(test)]
#[path = "wrap_language_model.test.rs"]
mod tests;
