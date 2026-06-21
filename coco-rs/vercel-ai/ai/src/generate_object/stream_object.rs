//! Stream structured object from a prompt.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::Usage;

use crate::error::AIError;
use crate::generate_text::build_call_options::apply_call_settings;
use crate::model::LanguageModel;
use crate::model::resolve_language_model;
use crate::prompt::CallSettings;
use crate::prompt::Prompt;
use crate::types::ProviderOptions;
use vercel_ai_provider::ProviderMetadata;

use super::ObjectGenerationMode;

/// Shared finish state populated by the inner task when the stream ends.
#[derive(Debug, Clone)]
struct StreamObjectFinishShared {
    pub usage: Usage,
    pub finish_reason: FinishReason,
    pub warnings: Vec<vercel_ai_provider::Warning>,
    pub response: Option<crate::types::LanguageModelResponseMetadata>,
    pub provider_metadata: Option<ProviderMetadata>,
}

/// A part of the object stream.
#[derive(Debug)]
pub enum ObjectStreamPart<T> {
    /// Partial object delta (for streaming).
    ObjectDelta {
        /// The partial object.
        delta: serde_json::Value,
    },
    /// Raw text delta from the model.
    TextDelta {
        /// The text delta.
        delta: String,
    },
    /// Complete object.
    Object {
        /// The complete object.
        object: T,
    },
    /// Error occurred.
    Error {
        /// The error.
        error: AIError,
    },
    /// Finish event.
    Finish {
        /// Token usage.
        usage: Usage,
        /// The finish reason from the model.
        finish_reason: FinishReason,
    },
}

/// Event emitted when stream_object finishes.
#[derive(Debug, Clone)]
pub struct StreamObjectFinishEvent {
    /// Token usage.
    pub usage: Usage,
    /// The finish reason from the model.
    pub finish_reason: FinishReason,
    /// The raw text output from the model.
    pub raw_text: String,
    /// Warnings from the provider.
    pub warnings: Vec<vercel_ai_provider::Warning>,
    /// The parsed final object as JSON (if parsing succeeded).
    pub object_json: Option<serde_json::Value>,
    /// Error message if object parsing failed.
    pub error: Option<String>,
    /// Response metadata from the provider.
    pub response: Option<crate::types::LanguageModelResponseMetadata>,
    /// Provider-specific metadata from the finish event.
    pub provider_metadata: Option<ProviderMetadata>,
}

/// Options for `stream_object`.
#[derive(Default)]
pub struct StreamObjectOptions<T> {
    /// The model to use.
    pub model: LanguageModel,
    /// The prompt to send to the model.
    pub prompt: Prompt,
    /// The JSON schema for the output.
    pub schema: vercel_ai_provider::JSONSchema,
    /// Optional name for the schema.
    pub schema_name: Option<String>,
    /// Optional description for the schema.
    pub schema_description: Option<String>,
    /// The mode for structured output.
    pub mode: ObjectGenerationMode,
    /// Call settings.
    pub settings: CallSettings,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Callback called when generation finishes.
    #[allow(clippy::type_complexity)]
    pub on_finish: Option<Arc<dyn Fn(&StreamObjectFinishEvent) + Send + Sync>>,
    /// Callback called when an error occurs.
    #[allow(clippy::type_complexity)]
    pub on_error: Option<Arc<dyn Fn(&AIError) + Send + Sync>>,
    /// Phantom data for the output type.
    _phantom: std::marker::PhantomData<T>,
}

impl<T> StreamObjectOptions<T> {
    /// Create new options with a model, prompt, and schema.
    pub fn new(
        model: impl Into<LanguageModel>,
        prompt: impl Into<Prompt>,
        schema: vercel_ai_provider::JSONSchema,
    ) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            schema,
            schema_name: None,
            schema_description: None,
            mode: ObjectGenerationMode::Auto,
            settings: CallSettings::default(),
            abort_signal: None,
            provider_options: None,
            on_finish: None,
            on_error: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set the schema name.
    pub fn with_schema_name(mut self, name: impl Into<String>) -> Self {
        self.schema_name = Some(name.into());
        self
    }

    /// Set the schema description.
    pub fn with_schema_description(mut self, description: impl Into<String>) -> Self {
        self.schema_description = Some(description.into());
        self
    }

    /// Set the generation mode.
    pub fn with_mode(mut self, mode: ObjectGenerationMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the call settings.
    pub fn with_settings(mut self, settings: CallSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the on_finish callback.
    pub fn with_on_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(&StreamObjectFinishEvent) + Send + Sync + 'static,
    {
        self.on_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_error callback.
    pub fn with_on_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(&AIError) + Send + Sync + 'static,
    {
        self.on_error = Some(Arc::new(callback));
        self
    }
}

/// Result of `stream_object`.
pub struct StreamObjectResult<T> {
    /// The stream of object parts.
    pub stream: Pin<Box<dyn Stream<Item = ObjectStreamPart<T>> + Send>>,
    /// Watch channel for finish data (populated when stream ends).
    finish_rx: tokio::sync::watch::Receiver<Option<StreamObjectFinishShared>>,
}

impl<T: Send + 'static> StreamObjectResult<T> {
    /// Collect the stream into a final object.
    pub async fn into_object(mut self) -> Result<T, AIError>
    where
        T: DeserializeOwned,
    {
        while let Some(part) = self.stream.next().await {
            match part {
                ObjectStreamPart::Object { object } => return Ok(object),
                ObjectStreamPart::Error { error } => return Err(error),
                _ => {}
            }
        }
        Err(AIError::NoOutputGenerated)
    }

    /// Wait for the stream to finish and return the token usage.
    pub async fn usage(&mut self) -> Usage {
        self.wait_for_finish().await.usage
    }

    /// Wait for the stream to finish and return the finish reason.
    pub async fn finish_reason(&mut self) -> FinishReason {
        self.wait_for_finish().await.finish_reason
    }

    /// Wait for the stream to finish and return any warnings.
    pub async fn warnings(&mut self) -> Vec<vercel_ai_provider::Warning> {
        self.wait_for_finish().await.warnings
    }

    /// Wait for the finish data to be available.
    async fn wait_for_finish(&mut self) -> StreamObjectFinishShared {
        // Wait until the watch has a value
        loop {
            {
                let val = self.finish_rx.borrow();
                if let Some(ref shared) = *val {
                    return shared.clone();
                }
            }
            if self.finish_rx.changed().await.is_err() {
                // Sender dropped — return defaults
                return StreamObjectFinishShared {
                    usage: Usage::default(),
                    finish_reason: FinishReason::end_turn(),
                    warnings: Vec::new(),
                    response: None,
                    provider_metadata: None,
                };
            }
        }
    }

    /// Wait for the stream to finish and return the response metadata.
    pub async fn response(&mut self) -> Option<crate::types::LanguageModelResponseMetadata> {
        self.wait_for_finish().await.response
    }

    /// Wait for the stream to finish and return provider-specific metadata.
    pub async fn provider_metadata(&mut self) -> Option<ProviderMetadata> {
        self.wait_for_finish().await.provider_metadata
    }

    /// Drain the stream and return the final object (non-consuming).
    pub async fn object(&mut self) -> Result<T, AIError>
    where
        T: DeserializeOwned,
    {
        while let Some(part) = self.stream.next().await {
            match part {
                ObjectStreamPart::Object { object } => return Ok(object),
                ObjectStreamPart::Error { error } => return Err(error),
                _ => {}
            }
        }
        Err(AIError::NoOutputGenerated)
    }

    /// Create a stream of deduped partial objects.
    ///
    /// Returns a stream of `serde_json::Value` that only emits when the
    /// partial object has changed (deep-equal deduplication).
    pub fn partial_object_stream(self) -> Pin<Box<dyn Stream<Item = serde_json::Value> + Send>> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut stream = self.stream;
            while let Some(part) = stream.next().await {
                if let ObjectStreamPart::ObjectDelta { delta } = part
                    && tx.send(delta).await.is_err()
                {
                    break;
                }
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }

    /// Create a stream of raw text deltas.
    ///
    /// Returns a stream of `String` values, one per text delta from the model.
    pub fn text_stream(self) -> Pin<Box<dyn Stream<Item = String> + Send>> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut stream = self.stream;
            while let Some(part) = stream.next().await {
                if let ObjectStreamPart::TextDelta { delta } = part
                    && tx.send(delta).await.is_err()
                {
                    break;
                }
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

/// Stream a structured object from a prompt.
///
/// This function streams structured output generation.
///
/// # Arguments
///
/// * `options` - The streaming options including model, prompt, and schema.
///
/// # Returns
///
/// A `StreamObjectResult<T>` containing the stream of object parts.
#[tracing::instrument(skip_all)]
pub fn stream_object<T: DeserializeOwned + Send + 'static>(
    options: StreamObjectOptions<T>,
) -> StreamObjectResult<T> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(None);

    tokio::spawn(async move {
        if let Err(e) = stream_object_inner(options, tx.clone(), watch_tx).await {
            let _ = tx.send(ObjectStreamPart::Error { error: e }).await;
        }
    });

    let stream = ReceiverStream::new(rx);
    StreamObjectResult {
        stream: Box::pin(stream),
        finish_rx: watch_rx,
    }
}

async fn stream_object_inner<T: DeserializeOwned + Send + 'static>(
    options: StreamObjectOptions<T>,
    tx: tokio::sync::mpsc::Sender<ObjectStreamPart<T>>,
    watch_tx: tokio::sync::watch::Sender<Option<StreamObjectFinishShared>>,
) -> Result<(), AIError> {
    let on_finish = options.on_finish.clone();
    let on_error = options.on_error.clone();
    let model = resolve_language_model(options.model)?;
    let messages = options.prompt.to_model_prompt();

    // Build call options based on mode
    let call_options = match options.mode {
        ObjectGenerationMode::Tool => {
            let tool_name = options
                .schema_name
                .clone()
                .unwrap_or_else(|| "json_output".to_string());

            let mut func_tool =
                crate::types::LanguageModelV4FunctionTool::new(&tool_name, options.schema.clone());
            func_tool.description = Some(
                options
                    .schema_description
                    .clone()
                    .unwrap_or_else(|| "Generate structured output".to_string()),
            );

            let tool = vercel_ai_provider::LanguageModelV4Tool::function(func_tool);

            let mut call_options = LanguageModelV4CallOptions::new(messages);
            call_options.tools = Some(vec![tool]);
            call_options.tool_choice =
                Some(vercel_ai_provider::LanguageModelV4ToolChoice::required());
            apply_call_settings(&mut call_options, &options.settings);
            if let Some(ref provider_opts) = options.provider_options {
                call_options.provider_options = Some(provider_opts.clone());
            }

            call_options
        }
        _ => {
            let response_format = ResponseFormat::json_with_schema(options.schema.clone())
                .with_name(
                    options
                        .schema_name
                        .clone()
                        .unwrap_or_else(|| "output".to_string()),
                );

            let mut call_options = LanguageModelV4CallOptions::new(messages);
            call_options.response_format = Some(response_format);
            apply_call_settings(&mut call_options, &options.settings);
            if let Some(ref provider_opts) = options.provider_options {
                call_options.provider_options = Some(provider_opts.clone());
            }

            call_options
        }
    };

    let stream_result = model
        .do_stream(&call_options, options.abort_signal.clone())
        .await?;

    // Capture response metadata from the stream result
    let response_metadata = stream_result.response.as_ref().map(|r| {
        let mut meta = crate::types::LanguageModelResponseMetadata::new();
        if let Some(ref headers) = r.headers {
            meta.headers = Some(headers.clone());
        }
        meta
    });

    // Process the stream
    let mut full_text = String::new();
    let mut usage = Usage::default();
    let mut finish_reason = FinishReason::end_turn();
    let mut finish_provider_metadata: Option<ProviderMetadata> = None;
    let mut stream = stream_result.stream;
    let mut last_partial: Option<serde_json::Value> = None;

    while let Some(part_result) = stream.next().await {
        match part_result {
            Ok(part) => match part {
                vercel_ai_provider::LanguageModelV4StreamPart::TextDelta { delta, .. } => {
                    full_text.push_str(&delta);

                    // Emit raw text delta
                    let _ = tx
                        .send(ObjectStreamPart::TextDelta {
                            delta: delta.clone(),
                        })
                        .await;

                    if let Some(partial) = crate::util::parse_partial_json(&full_text) {
                        // Dedup: only emit if the partial object changed
                        let should_emit = last_partial
                            .as_ref()
                            .is_none_or(|prev| !crate::util::is_deep_equal(prev, &partial));
                        if should_emit {
                            last_partial = Some(partial.clone());
                            let _ = tx
                                .send(ObjectStreamPart::ObjectDelta { delta: partial })
                                .await;
                        }
                    }
                }
                vercel_ai_provider::LanguageModelV4StreamPart::ToolInputDelta { delta, .. } => {
                    // In Tool mode, tool input deltas contain the JSON
                    full_text.push_str(&delta);

                    // Emit raw text delta
                    let _ = tx
                        .send(ObjectStreamPart::TextDelta {
                            delta: delta.clone(),
                        })
                        .await;

                    if let Some(partial) = crate::util::parse_partial_json(&full_text) {
                        let should_emit = last_partial
                            .as_ref()
                            .is_none_or(|prev| !crate::util::is_deep_equal(prev, &partial));
                        if should_emit {
                            last_partial = Some(partial.clone());
                            let _ = tx
                                .send(ObjectStreamPart::ObjectDelta { delta: partial })
                                .await;
                        }
                    }
                }
                vercel_ai_provider::LanguageModelV4StreamPart::Finish {
                    usage: u,
                    finish_reason: fr,
                    provider_metadata: pm,
                } => {
                    usage = u;
                    finish_reason = fr;
                    finish_provider_metadata = pm;
                }
                _ => {}
            },
            Err(e) => {
                let error = AIError::ProviderError(e);
                if let Some(ref cb) = on_error {
                    cb(&error);
                }
                let _ = tx.send(ObjectStreamPart::Error { error }).await;
                return Ok(());
            }
        }
    }

    // Send finish data via watch channel (before parsing, so waiters get notified)
    let _ = watch_tx.send(Some(StreamObjectFinishShared {
        usage: usage.clone(),
        finish_reason: finish_reason.clone(),
        warnings: Vec::new(),
        response: response_metadata.clone(),
        provider_metadata: finish_provider_metadata.clone(),
    }));

    // Parse the final object
    match serde_json::from_str::<T>(&full_text) {
        Ok(object) => {
            // Try to parse as JSON Value for the finish event
            let object_json = serde_json::from_str::<serde_json::Value>(&full_text).ok();
            let _ = tx.send(ObjectStreamPart::Object { object }).await;
            let _ = tx
                .send(ObjectStreamPart::Finish {
                    usage: usage.clone(),
                    finish_reason: finish_reason.clone(),
                })
                .await;

            // Call on_finish callback
            if let Some(ref cb) = on_finish {
                cb(&StreamObjectFinishEvent {
                    usage,
                    finish_reason,
                    raw_text: full_text,
                    warnings: Vec::new(),
                    object_json,
                    error: None,
                    response: response_metadata,
                    provider_metadata: finish_provider_metadata,
                });
            }
        }
        Err(e) => {
            let error_msg = format!("Failed to parse JSON: {e}");
            let error = AIError::SchemaValidation(error_msg.clone());
            if let Some(ref cb) = on_error {
                cb(&error);
            }
            let _ = tx.send(ObjectStreamPart::Error { error }).await;

            // Call on_finish callback even on parse failure
            if let Some(ref cb) = on_finish {
                cb(&StreamObjectFinishEvent {
                    usage,
                    finish_reason,
                    raw_text: full_text,
                    warnings: Vec::new(),
                    object_json: None,
                    error: Some(error_msg),
                    response: response_metadata,
                    provider_metadata: finish_provider_metadata,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "stream_object.test.rs"]
mod tests;
