//! Language model middleware trait (V4).
//!
//! This module defines middleware patterns for language models following
//! the Vercel AI SDK v4 specification.
//!
//! Middleware provides fine-grained hooks to intercept and modify model behavior:
//! - `override_provider`: Change the provider name
//! - `override_model_id`: Change the model ID
//! - `override_supported_urls`: Change the supported URLs
//! - `transform_params`: Transform call parameters before the call
//! - `wrap_generate`: Wrap the generate call
//! - `wrap_stream`: Wrap the stream call

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::language_model::LanguageModelV4;
use crate::language_model::LanguageModelV4CallOptions;
use crate::language_model::LanguageModelV4GenerateResult;
use crate::language_model::LanguageModelV4StreamResult;

/// Type alias for a boxed future.
type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Options passed to middleware hooks.
pub struct MiddlewareOptions {
    /// The model being wrapped.
    pub model: Arc<dyn LanguageModelV4>,
}

/// Options for transform_params hook.
pub struct TransformParamsOptions {
    /// The type of call: 'generate' or 'stream'.
    pub call_type: CallType,
    /// The parameters to transform.
    pub params: LanguageModelV4CallOptions,
    /// The model being called.
    pub model: Arc<dyn LanguageModelV4>,
}

/// Type of call being made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
    /// A generate (non-streaming) call.
    Generate,
    /// A streaming call.
    Stream,
}

/// Options for wrap_generate hook.
///
/// `params` is **owned** at the middleware layer because `transform_params`
/// may produce a fresh modified copy. The middleware's `do_generate` closure
/// receives the owned params + abort signal and is expected to forward
/// `&params` to the leaf provider (zero further clone).
pub struct WrapGenerateOptions {
    /// The transformed parameters for the call.
    pub params: LanguageModelV4CallOptions,
    /// The cancellation handle for this call (live, not part of the spec).
    pub abort_signal: Option<CancellationToken>,
    /// The model being called.
    pub model: Arc<dyn LanguageModelV4>,
    /// The function to call the next middleware or the actual model.
    pub do_generate: DoGenerateFn,
}

/// Closure signature used by middleware to call into the next layer in
/// the chain for a generate request.
#[allow(clippy::type_complexity)]
pub type DoGenerateFn = Box<
    dyn FnOnce(
            LanguageModelV4CallOptions,
            Option<CancellationToken>,
        ) -> BoxFuture<Result<LanguageModelV4GenerateResult, AISdkError>>
        + Send,
>;

/// Options for wrap_stream hook.
///
/// See [`WrapGenerateOptions`] for the ownership rationale.
pub struct WrapStreamOptions {
    /// The transformed parameters for the call.
    pub params: LanguageModelV4CallOptions,
    /// The cancellation handle for this call (live, not part of the spec).
    pub abort_signal: Option<CancellationToken>,
    /// The model being called.
    pub model: Arc<dyn LanguageModelV4>,
    /// The function to call the next middleware or the actual model.
    pub do_stream: DoStreamFn,
}

/// Closure signature used by middleware to call into the next layer in
/// the chain for a stream request.
#[allow(clippy::type_complexity)]
pub type DoStreamFn = Box<
    dyn FnOnce(
            LanguageModelV4CallOptions,
            Option<CancellationToken>,
        ) -> BoxFuture<Result<LanguageModelV4StreamResult, AISdkError>>
        + Send,
>;

/// Trait for language model middleware (V4).
///
/// Middleware can intercept and modify calls to language models,
/// enabling cross-cutting concerns like logging, caching, rate limiting, etc.
///
/// Each hook is optional - implement only the hooks you need.
#[async_trait::async_trait]
pub trait LanguageModelV4Middleware: Send + Sync {
    /// Override the provider name.
    ///
    /// This is called to potentially change the provider name returned by the model.
    fn override_provider(&self, _options: &MiddlewareOptions) -> Option<String> {
        None
    }

    /// Override the model ID.
    ///
    /// This is called to potentially change the model ID returned by the model.
    fn override_model_id(&self, _options: &MiddlewareOptions) -> Option<String> {
        None
    }

    /// Override the supported URLs.
    ///
    /// This is called to potentially change the supported URLs returned by the model.
    fn override_supported_urls(
        &self,
        _options: &MiddlewareOptions,
    ) -> Option<HashMap<String, Vec<regex::Regex>>> {
        None
    }

    /// Transform parameters before a call.
    ///
    /// This is called before `do_generate` or `do_stream` to allow modifying
    /// the parameters (e.g., adding headers, modifying prompts, etc.).
    async fn transform_params(
        &self,
        options: TransformParamsOptions,
    ) -> Result<LanguageModelV4CallOptions, AISdkError> {
        Ok(options.params)
    }

    /// Wrap a generate call.
    ///
    /// This allows intercepting the generate call for logging, caching, etc.
    /// The default implementation just calls through to the model.
    async fn wrap_generate(
        &self,
        options: WrapGenerateOptions,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        (options.do_generate)(options.params, options.abort_signal).await
    }

    /// Wrap a stream call.
    ///
    /// This allows intercepting the stream call for logging, caching, etc.
    /// The default implementation just calls through to the model.
    async fn wrap_stream(
        &self,
        options: WrapStreamOptions,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        (options.do_stream)(options.params, options.abort_signal).await
    }
}

/// A wrapper that applies middleware to a language model.
pub struct MiddlewareWrapper {
    inner: Arc<dyn LanguageModelV4>,
    middleware: Arc<dyn LanguageModelV4Middleware>,
    /// Cached overridden provider name.
    provider_override: Option<String>,
    /// Cached overridden model ID.
    model_id_override: Option<String>,
}

impl MiddlewareWrapper {
    /// Create a new middleware wrapper.
    pub fn new(
        inner: Arc<dyn LanguageModelV4>,
        middleware: Arc<dyn LanguageModelV4Middleware>,
    ) -> Self {
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
            Some(s) => s,
            None => self.inner.provider(),
        }
    }

    fn model_id(&self) -> &str {
        match &self.model_id_override {
            Some(s) => s,
            None => self.inner.model_id(),
        }
    }

    fn supported_urls(&self) -> HashMap<String, Vec<regex::Regex>> {
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
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        // Transform params — middleware may produce a fresh modified copy.
        let transform_options = TransformParamsOptions {
            call_type: CallType::Generate,
            params: params.clone(),
            model: self.inner.clone(),
        };
        let transformed_params = self.middleware.transform_params(transform_options).await?;

        // Wrap generate. The closure receives owned `params` + `abort_signal`
        // and forwards `&params` to the leaf provider (zero further clone).
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
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
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

/// A chain of middleware that can be applied to a language model.
pub struct MiddlewareChain {
    middlewares: Vec<Arc<dyn LanguageModelV4Middleware>>,
}

impl MiddlewareChain {
    /// Create a new empty middleware chain.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the chain.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, middleware: Arc<dyn LanguageModelV4Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// Apply the middleware chain to a model.
    pub fn apply(&self, mut model: Arc<dyn LanguageModelV4>) -> Arc<dyn LanguageModelV4> {
        for middleware in &self.middlewares {
            model = Arc::new(MiddlewareWrapper::new(model, middleware.clone()));
        }
        model
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "language_model_v4_middleware.test.rs"]
mod tests;
