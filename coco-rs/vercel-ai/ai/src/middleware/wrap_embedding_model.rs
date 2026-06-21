//! Wrap an embedding model with middleware.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::EmbeddingModelV4CallOptions;
use vercel_ai_provider::EmbeddingModelV4EmbedResult;

/// Trait for embedding model middleware (extended version).
#[async_trait::async_trait]
pub trait EmbeddingMiddleware: Send + Sync {
    /// Transform parameters before the embed call.
    async fn transform_params(
        &self,
        params: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4CallOptions, AISdkError> {
        Ok(params)
    }
}

/// Wrapper for embedding model middleware.
struct EmbeddingMiddlewareWrapper {
    model: Arc<dyn EmbeddingModelV4>,
    middleware: Arc<dyn EmbeddingMiddleware>,
}

#[async_trait::async_trait]
impl EmbeddingModelV4 for EmbeddingMiddlewareWrapper {
    fn provider(&self) -> &str {
        self.model.provider()
    }

    fn model_id(&self) -> &str {
        self.model.model_id()
    }

    fn max_embeddings_per_call(&self) -> usize {
        self.model.max_embeddings_per_call()
    }

    fn supports_parallel_calls(&self) -> bool {
        self.model.supports_parallel_calls()
    }

    async fn do_embed(
        &self,
        params: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4EmbedResult, AISdkError> {
        let transformed_params = self.middleware.transform_params(params).await?;
        self.model.do_embed(transformed_params).await
    }
}

/// Wrap an embedding model with middleware.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::middleware::wrap_embedding_model;
/// use std::sync::Arc;
///
/// let wrapped = wrap_embedding_model(model, middleware);
/// ```
pub fn wrap_embedding_model(
    model: Arc<dyn EmbeddingModelV4>,
    middleware: Arc<dyn EmbeddingMiddleware>,
) -> Arc<dyn EmbeddingModelV4> {
    Arc::new(EmbeddingMiddlewareWrapper { model, middleware })
}

#[cfg(test)]
#[path = "wrap_embedding_model.test.rs"]
mod tests;
