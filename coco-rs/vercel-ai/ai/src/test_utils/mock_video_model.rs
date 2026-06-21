//! Mock video model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_provider::VideoModelV4Result;

type GenerateHandler =
    Arc<dyn Fn(VideoModelV4CallOptions) -> Result<VideoModelV4Result, AISdkError> + Send + Sync>;

/// A configurable mock video model for testing.
pub struct MockVideoModel {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
    call_log: Arc<Mutex<Vec<VideoModelV4CallOptions>>>,
}

impl MockVideoModel {
    /// Create a builder for a mock video model.
    pub fn builder() -> MockVideoModelBuilder {
        MockVideoModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<VideoModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl VideoModelV4 for MockVideoModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate_video(
        &self,
        options: VideoModelV4CallOptions,
    ) -> Result<VideoModelV4Result, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.generate_handler {
            handler(options)
        } else {
            // Default: return a single dummy video URL
            Ok(VideoModelV4Result::from_urls(vec![
                "https://example.com/video.mp4".to_string(),
            ]))
        }
    }
}

/// Builder for `MockVideoModel`.
pub struct MockVideoModelBuilder {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
}

impl MockVideoModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-video".to_string(),
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

    /// Set a custom generate handler.
    pub fn with_generate_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(VideoModelV4CallOptions) -> Result<VideoModelV4Result, AISdkError>
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
    pub fn build(self) -> MockVideoModel {
        MockVideoModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            generate_handler: self.generate_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockVideoModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
