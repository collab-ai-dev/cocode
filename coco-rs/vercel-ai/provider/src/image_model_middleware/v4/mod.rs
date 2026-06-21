//! Image model middleware trait (V4).
//!
//! This module defines middleware patterns for image models.

use std::sync::Arc;

use crate::image_model::ImageModelV4;

/// Trait for image model middleware.
///
/// Middleware can intercept and modify calls to image models,
/// enabling cross-cutting concerns like logging, caching, rate limiting, etc.
pub trait ImageModelV4Middleware: Send + Sync {
    /// Wrap an image model with this middleware.
    fn wrap(&self, model: Arc<dyn ImageModelV4>) -> Arc<dyn ImageModelV4>;
}

/// A chain of middleware that can be applied to an image model.
pub struct ImageMiddlewareChain {
    middlewares: Vec<Arc<dyn ImageModelV4Middleware>>,
}

impl ImageMiddlewareChain {
    /// Create a new empty middleware chain.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the chain.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, middleware: Arc<dyn ImageModelV4Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// Apply the middleware chain to a model.
    pub fn apply(&self, mut model: Arc<dyn ImageModelV4>) -> Arc<dyn ImageModelV4> {
        for middleware in &self.middlewares {
            model = middleware.wrap(model);
        }
        model
    }
}

impl Default for ImageMiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "image_model_v4_middleware.test.rs"]
mod tests;
