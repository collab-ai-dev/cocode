//! Wrap an image model with middleware.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageModelV4GenerateResult;

/// Trait for image model middleware (extended version).
#[async_trait::async_trait]
pub trait ImageMiddleware: Send + Sync {
    /// Transform parameters before the generate call.
    async fn transform_params(
        &self,
        params: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4CallOptions, AISdkError> {
        Ok(params)
    }
}

/// Wrapper for image model middleware.
struct ImageMiddlewareWrapper {
    model: Arc<dyn ImageModelV4>,
    middleware: Arc<dyn ImageMiddleware>,
}

#[async_trait::async_trait]
impl ImageModelV4 for ImageMiddlewareWrapper {
    fn provider(&self) -> &str {
        self.model.provider()
    }

    fn model_id(&self) -> &str {
        self.model.model_id()
    }

    fn max_images_per_call(&self) -> usize {
        self.model.max_images_per_call()
    }

    async fn do_generate(
        &self,
        params: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError> {
        let transformed_params = self.middleware.transform_params(params).await?;
        self.model.do_generate(transformed_params).await
    }
}

/// Wrap an image model with middleware.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::middleware::wrap_image_model;
/// use std::sync::Arc;
///
/// let wrapped = wrap_image_model(model, middleware);
/// ```
pub fn wrap_image_model(
    model: Arc<dyn ImageModelV4>,
    middleware: Arc<dyn ImageMiddleware>,
) -> Arc<dyn ImageModelV4> {
    Arc::new(ImageMiddlewareWrapper { model, middleware })
}

#[cfg(test)]
#[path = "wrap_image_model.test.rs"]
mod tests;
