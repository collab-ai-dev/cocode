//! Mock speech model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::SpeechModelV4CallOptions;
use vercel_ai_provider::SpeechModelV4Result;

type GenerateHandler =
    Arc<dyn Fn(SpeechModelV4CallOptions) -> Result<SpeechModelV4Result, AISdkError> + Send + Sync>;

/// A configurable mock speech model for testing.
pub struct MockSpeechModel {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
    call_log: Arc<Mutex<Vec<SpeechModelV4CallOptions>>>,
}

impl MockSpeechModel {
    /// Create a builder for a mock speech model.
    pub fn builder() -> MockSpeechModelBuilder {
        MockSpeechModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<SpeechModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl SpeechModelV4 for MockSpeechModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate_speech(
        &self,
        options: SpeechModelV4CallOptions,
    ) -> Result<SpeechModelV4Result, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.generate_handler {
            handler(options)
        } else {
            // Default: return dummy MP3 audio bytes with full result
            Ok(SpeechModelV4Result::mp3(vec![0xFF, 0xFB, 0x90, 0x00]))
        }
    }
}

/// Builder for `MockSpeechModel`.
pub struct MockSpeechModelBuilder {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
}

impl MockSpeechModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-speech".to_string(),
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
        F: Fn(SpeechModelV4CallOptions) -> Result<SpeechModelV4Result, AISdkError>
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
    pub fn build(self) -> MockSpeechModel {
        MockSpeechModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            generate_handler: self.generate_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockSpeechModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
