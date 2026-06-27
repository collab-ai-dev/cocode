//! Embedding model middleware trait (V4).
//!
//! This module defines middleware patterns for embedding models.

use std::sync::Arc;

use crate::embedding_model::EmbeddingModelV4;

/// Trait for embedding model middleware.
///
/// Middleware can intercept and modify calls to embedding models,
/// enabling cross-cutting concerns like logging, caching, rate limiting, etc.
pub trait EmbeddingModelV4Middleware: Send + Sync {
    /// Wrap an embedding model with this middleware.
    fn wrap(&self, model: Arc<dyn EmbeddingModelV4>) -> Arc<dyn EmbeddingModelV4>;
}

/// A chain of middleware that can be applied to an embedding model.
pub struct EmbeddingMiddlewareChain {
    middlewares: Vec<Arc<dyn EmbeddingModelV4Middleware>>,
}

impl EmbeddingMiddlewareChain {
    /// Create a new empty middleware chain.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the chain.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, middleware: Arc<dyn EmbeddingModelV4Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// Apply the middleware chain to a model.
    pub fn apply(&self, mut model: Arc<dyn EmbeddingModelV4>) -> Arc<dyn EmbeddingModelV4> {
        for middleware in &self.middlewares {
            model = middleware.wrap(model);
        }
        model
    }
}

impl Default for EmbeddingMiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "embedding_model_v4_middleware.test.rs"]
mod tests;
