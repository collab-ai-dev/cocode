//! Mock transcription model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;
use vercel_ai_provider::TranscriptionModelV4Result;

type TranscribeHandler = Arc<
    dyn Fn(TranscriptionModelV4CallOptions) -> Result<TranscriptionModelV4Result, AISdkError>
        + Send
        + Sync,
>;

/// A configurable mock transcription model for testing.
pub struct MockTranscriptionModel {
    provider_name: String,
    model_id: String,
    transcribe_handler: Option<TranscribeHandler>,
    call_log: Arc<Mutex<Vec<TranscriptionModelV4CallOptions>>>,
}

impl MockTranscriptionModel {
    /// Create a builder for a mock transcription model.
    pub fn builder() -> MockTranscriptionModelBuilder {
        MockTranscriptionModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<TranscriptionModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl TranscriptionModelV4 for MockTranscriptionModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_transcribe(
        &self,
        options: TranscriptionModelV4CallOptions,
    ) -> Result<TranscriptionModelV4Result, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.transcribe_handler {
            handler(options)
        } else {
            // Default: return dummy transcription with full result
            Ok(TranscriptionModelV4Result::new("Hello, world!"))
        }
    }
}

/// Builder for `MockTranscriptionModel`.
pub struct MockTranscriptionModelBuilder {
    provider_name: String,
    model_id: String,
    transcribe_handler: Option<TranscribeHandler>,
}

impl MockTranscriptionModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-transcription".to_string(),
            transcribe_handler: None,
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

    /// Set a custom transcribe handler.
    pub fn with_transcribe_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(TranscriptionModelV4CallOptions) -> Result<TranscriptionModelV4Result, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.transcribe_handler = Some(Arc::new(handler));
        self
    }

    /// Set a handler that returns a specific text.
    pub fn with_text_response(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.with_transcribe_handler(move |_| Ok(TranscriptionModelV4Result::new(text.clone())))
    }

    /// Set a handler that returns an error.
    pub fn with_error(self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.with_transcribe_handler(move |_| Err(AISdkError::new(&error)))
    }

    /// Build the mock model.
    pub fn build(self) -> MockTranscriptionModel {
        MockTranscriptionModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            transcribe_handler: self.transcribe_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockTranscriptionModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
