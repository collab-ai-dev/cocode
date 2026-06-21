//! Mock image model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageModelV4GenerateResult;

type GenerateHandler = Arc<
    dyn Fn(ImageModelV4CallOptions) -> Result<ImageModelV4GenerateResult, AISdkError> + Send + Sync,
>;

/// A configurable mock image model for testing.
pub struct MockImageModel {
    provider_name: String,
    model_id: String,
    max_images: usize,
    generate_handler: Option<GenerateHandler>,
    call_log: Arc<Mutex<Vec<ImageModelV4CallOptions>>>,
}

impl MockImageModel {
    /// Create a builder for a mock image model.
    pub fn builder() -> MockImageModelBuilder {
        MockImageModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<ImageModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl ImageModelV4 for MockImageModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_images_per_call(&self) -> usize {
        self.max_images
    }

    async fn do_generate(
        &self,
        options: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.generate_handler {
            handler(options)
        } else {
            // Default: return a single dummy base64 image
            Ok(ImageModelV4GenerateResult::from_base64(vec![
                "iVBORw0KGgo=".to_string(),
            ]))
        }
    }
}

/// Builder for `MockImageModel`.
pub struct MockImageModelBuilder {
    provider_name: String,
    model_id: String,
    max_images: usize,
    generate_handler: Option<GenerateHandler>,
}

impl MockImageModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-image".to_string(),
            max_images: 1,
            generate_handler: None,
        }
    }

    /// Set the provider name.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider_name = provider.into();
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Set the max images per call.
    pub fn with_max_images(mut self, max: usize) -> Self {
        self.max_images = max;
        self
    }

    /// Set a custom generate handler.
    pub fn with_generate_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(ImageModelV4CallOptions) -> Result<ImageModelV4GenerateResult, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.generate_handler = Some(Arc::new(handler));
        self
    }

    /// Set a handler that returns an error.
    pub fn with_error(self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.with_generate_handler(move |_| Err(AISdkError::new(&error)))
    }

    /// Build the mock model.
    pub fn build(self) -> MockImageModel {
        MockImageModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            max_images: self.max_images,
            generate_handler: self.generate_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockImageModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
