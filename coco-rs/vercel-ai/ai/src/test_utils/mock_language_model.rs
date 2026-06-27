//! Mock language model for testing.
//!
//! Provides a configurable mock implementation of `LanguageModelV4` for use in tests.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use futures::Stream;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::Usage;

type GenerateHandler = Arc<
    dyn Fn(LanguageModelV4CallOptions) -> Result<LanguageModelV4GenerateResult, AISdkError>
        + Send
        + Sync,
>;

type StreamHandler = Arc<
    dyn Fn(LanguageModelV4CallOptions) -> Result<LanguageModelV4StreamResult, AISdkError>
        + Send
        + Sync,
>;

/// A configurable mock language model for testing.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::test_utils::MockLanguageModel;
///
/// let model = MockLanguageModel::builder()
///     .with_text_response("Hello, world!")
///     .build();
///
/// let result = generate_text(GenerateTextOptions {
///     model: LanguageModel::from_v4(Arc::new(model)),
///     prompt: Prompt::user("Hi"),
///     ..Default::default()
/// }).await?;
/// ```
pub struct MockLanguageModel {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
    stream_handler: Option<StreamHandler>,
    call_log: Arc<Mutex<Vec<LanguageModelV4CallOptions>>>,
    generate_calls: Arc<Mutex<Vec<LanguageModelV4CallOptions>>>,
    stream_calls: Arc<Mutex<Vec<LanguageModelV4CallOptions>>>,
}

impl MockLanguageModel {
    /// Create a new mock model that returns the given text.
    pub fn with_text(text: impl Into<String>) -> Self {
        MockLanguageModelBuilder::new()
            .with_text_response(text)
            .build()
    }

    /// Create a builder for a mock model.
    pub fn builder() -> MockLanguageModelBuilder {
        MockLanguageModelBuilder::new()
    }

    /// Get the call log (all calls made to do_generate/do_stream).
    pub fn calls(&self) -> Vec<LanguageModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }

    /// Get only the do_generate calls.
    pub fn generate_calls(&self) -> Vec<LanguageModelV4CallOptions> {
        self.generate_calls.lock().unwrap().clone()
    }

    /// Get the number of do_generate calls.
    pub fn generate_call_count(&self) -> usize {
        self.generate_calls.lock().unwrap().len()
    }

    /// Get only the do_stream calls.
    pub fn stream_calls(&self) -> Vec<LanguageModelV4CallOptions> {
        self.stream_calls.lock().unwrap().clone()
    }

    /// Get the number of do_stream calls.
    pub fn stream_call_count(&self) -> usize {
        self.stream_calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl LanguageModelV4 for MockLanguageModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());
        self.generate_calls.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.generate_handler {
            // Test handler retains the owned-options contract — clone here.
            handler(options.clone())
        } else {
            Ok(default_generate_result(""))
        }
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());
        self.stream_calls.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.stream_handler {
            handler(options.clone())
        } else {
            // Return an empty stream by default
            let stream: Pin<
                Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
            > = Box::pin(futures::stream::empty());

            Ok(LanguageModelV4StreamResult {
                stream,
                request: None,
                response: None,
            })
        }
    }
}

/// Builder for `MockLanguageModel`.
pub struct MockLanguageModelBuilder {
    provider_name: String,
    model_id: String,
    generate_handler: Option<GenerateHandler>,
    stream_handler: Option<StreamHandler>,
}

impl MockLanguageModelBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-model".to_string(),
            generate_handler: None,
            stream_handler: None,
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

    /// Set a simple text response for do_generate.
    pub fn with_text_response(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.with_generate_handler(move |_| Ok(default_generate_result(&text)))
    }

    /// Set multiple responses that are returned in order (for multi-step tests).
    ///
    /// Returns responses in sequence: first call returns responses[0],
    /// second call returns responses[1], etc. Wraps around if more calls
    /// than responses.
    pub fn with_responses(self, responses: Vec<LanguageModelV4GenerateResult>) -> Self {
        let counter = Arc::new(AtomicUsize::new(0));
        let responses = Arc::new(responses);
        self.with_generate_handler(move |_| {
            let idx = counter.fetch_add(1, Ordering::SeqCst);
            let response = responses[idx % responses.len()].clone();
            Ok(response)
        })
    }

    /// Set a response that includes a tool call.
    pub fn with_tool_call_response(
        self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        self.with_generate_handler(move |_| {
            Ok(LanguageModelV4GenerateResult {
                content: vec![AssistantContentPart::tool_call(
                    &tool_call_id,
                    &tool_name,
                    args.clone(),
                )],
                usage: Usage::new(10, 5),
                finish_reason: FinishReason::tool_use(),
                warnings: Vec::new(),
                provider_metadata: None,
                request: None,
                response: None,
            })
        })
    }

    /// Set a custom generate handler.
    pub fn with_generate_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(LanguageModelV4CallOptions) -> Result<LanguageModelV4GenerateResult, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.generate_handler = Some(Arc::new(handler));
        self
    }

    /// Set a custom stream handler.
    pub fn with_stream_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(LanguageModelV4CallOptions) -> Result<LanguageModelV4StreamResult, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.stream_handler = Some(Arc::new(handler));
        self
    }

    /// Set a stream handler that returns text parts.
    pub fn with_stream_text_response(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.with_stream_handler(move |_| {
            let parts = vec![
                Ok(LanguageModelV4StreamPart::TextDelta {
                    id: String::new(),
                    delta: text.clone(),
                    provider_metadata: None,
                }),
                Ok(LanguageModelV4StreamPart::Finish {
                    finish_reason: FinishReason::end_turn(),
                    usage: Usage::new(10, 5),
                    provider_metadata: None,
                }),
            ];
            let stream: Pin<
                Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
            > = Box::pin(futures::stream::iter(parts));

            Ok(LanguageModelV4StreamResult {
                stream,
                request: None,
                response: None,
            })
        })
    }

    /// Set a stream handler that returns a tool call.
    pub fn with_stream_tool_call_response(
        self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        self.with_stream_handler(move |_| {
            let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                &tool_call_id,
                &tool_name,
                args.clone(),
            );
            let parts = vec![
                Ok(LanguageModelV4StreamPart::ToolInputStart {
                    id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    provider_executed: None,
                    dynamic: None,
                    title: None,
                    provider_metadata: None,
                }),
                Ok(LanguageModelV4StreamPart::ToolInputDelta {
                    id: tool_call_id.clone(),
                    delta: serde_json::to_string(&args).unwrap_or_default(),
                    provider_metadata: None,
                }),
                Ok(LanguageModelV4StreamPart::ToolInputEnd {
                    id: tool_call_id.clone(),
                    provider_metadata: None,
                }),
                Ok(LanguageModelV4StreamPart::ToolCall(tc)),
                Ok(LanguageModelV4StreamPart::Finish {
                    finish_reason: FinishReason::tool_use(),
                    usage: Usage::new(10, 5),
                    provider_metadata: None,
                }),
            ];
            let stream: Pin<
                Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
            > = Box::pin(futures::stream::iter(parts));

            Ok(LanguageModelV4StreamResult {
                stream,
                request: None,
                response: None,
            })
        })
    }

    /// Set a generate handler that returns an error.
    pub fn with_error(self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.with_generate_handler(move |_| Err(AISdkError::new(&error)))
    }

    /// Build the mock model.
    pub fn build(self) -> MockLanguageModel {
        MockLanguageModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            generate_handler: self.generate_handler,
            stream_handler: self.stream_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
            generate_calls: Arc::new(Mutex::new(Vec::new())),
            stream_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockLanguageModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a default generate result with the given text.
pub fn default_generate_result(text: &str) -> LanguageModelV4GenerateResult {
    LanguageModelV4GenerateResult {
        content: vec![AssistantContentPart::text(text)],
        usage: Usage::new(10, 5),
        finish_reason: FinishReason::end_turn(),
        warnings: Vec::new(),
        provider_metadata: None,
        request: None,
        response: None,
    }
}
