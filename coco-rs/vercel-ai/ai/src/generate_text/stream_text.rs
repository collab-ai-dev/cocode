//! Stream text from a prompt.
//!
//! This module provides the `stream_text` function for streaming text
//! generation from a language model.

use std::any::Any;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Source;
use vercel_ai_provider::Usage;
use vercel_ai_provider::Warning;

use crate::error::AIError;
use crate::model::LanguageModel;
use crate::model::resolve_language_model;
use crate::prompt::CallSettings;
use crate::types::ProviderOptions;
use crate::types::ToolExecutionOptions;
use crate::types::ToolRegistry;
use crate::util::retry::RetryConfig;

use super::build_call_options;
use super::callback::CallbackModelInfo;
use super::callback::OnChunkEvent;
use super::callback::OnFinishEvent;
use super::callback::OnStartEvent;
use super::callback::OnStepStartEvent;
use super::callback::OnToolCallFinishEvent;
use super::callback::OnToolCallStartEvent;
use super::callback::StreamTextCallbacks;
use super::collect_tool_approvals::ToolApprovalCollector;
use super::collect_tool_approvals::ToolApprovalRequest;
use super::collect_tool_approvals::apply_approvals;
use super::content_utils;
use super::generate::PrepareStepContext;
use super::generate::PrepareStepFn;
use super::generate::RuntimeContext;
use super::generate::ToolContextMap;
use super::generate_text_result::GenerateTextResult;
use super::generate_text_result::ToolCall;
use super::generate_text_result::ToolResult;
use super::generated_file::GeneratedFile;
use super::output::Output;
use super::response_message::build_tool_result_message;
use super::step_result::StepResult;
use super::stop_condition::StopCondition;
use super::tool_error::ToolError;

/// Options for `stream_text`.
#[derive(Default)]
pub struct StreamTextOptions {
    /// The model to use.
    pub model: LanguageModel,
    /// The prompt to send to the model.
    pub prompt: crate::prompt::Prompt,
    /// Tools available to the model.
    pub tools: Option<Arc<ToolRegistry>>,
    /// Tool choice configuration.
    pub tool_choice: Option<LanguageModelV4ToolChoice>,
    /// Maximum number of steps for tool calling.
    pub max_steps: Option<u32>,
    /// Call settings.
    pub settings: CallSettings,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Callbacks for lifecycle events.
    pub callbacks: StreamTextCallbacks,
    /// Retry configuration for transient failures.
    pub retry_config: Option<RetryConfig>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Output configuration for structured output.
    pub output: Option<Output>,
    /// Filter which tools are available per step.
    pub active_tools: Option<Vec<String>>,
    /// Stop conditions for multi-step generation.
    pub stop_when: Vec<StopCondition>,
    /// Per-step overrides callback.
    pub prepare_step: Option<PrepareStepFn>,
    /// Runtime context shared with prepare_step and tool execution.
    pub runtime_context: Option<RuntimeContext>,
    /// Tool-specific runtime contexts keyed by tool name.
    pub tools_context: ToolContextMap,
    /// Tool approval collector.
    pub tool_call_approval: Option<Arc<dyn ToolApprovalCollector>>,
    /// Telemetry configuration.
    pub telemetry: Option<crate::telemetry::TelemetrySettings>,
}

impl StreamTextOptions {
    /// Create new options with a model and prompt.
    pub fn new(model: impl Into<LanguageModel>, prompt: impl Into<crate::prompt::Prompt>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// Set the tools registry.
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set the tool choice.
    pub fn with_tool_choice(mut self, choice: LanguageModelV4ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Set the maximum steps.
    pub fn with_max_steps(mut self, max_steps: u32) -> Self {
        self.max_steps = Some(max_steps);
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

    /// Set the callbacks.
    pub fn with_callbacks(mut self, callbacks: StreamTextCallbacks) -> Self {
        self.callbacks = callbacks;
        self
    }

    /// Set the retry configuration.
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = Some(config);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the output configuration for structured output.
    pub fn with_output(mut self, output: Output) -> Self {
        self.output = Some(output);
        self
    }

    /// Set the active tools filter.
    pub fn with_active_tools(mut self, tools: Vec<String>) -> Self {
        self.active_tools = Some(tools);
        self
    }

    /// Add a stop condition.
    pub fn with_stop_when(mut self, condition: StopCondition) -> Self {
        self.stop_when.push(condition);
        self
    }

    /// Set the prepare_step callback.
    pub fn with_prepare_step(mut self, prepare: PrepareStepFn) -> Self {
        self.prepare_step = Some(prepare);
        self
    }

    /// Set shared runtime context.
    pub fn with_runtime_context<T: Any + Send + Sync + 'static>(mut self, context: T) -> Self {
        self.runtime_context = Some(Arc::new(context));
        self
    }

    /// Set shared runtime context from an existing Arc.
    pub fn with_runtime_context_arc(mut self, context: RuntimeContext) -> Self {
        self.runtime_context = Some(context);
        self
    }

    /// Set runtime context for a specific tool.
    pub fn with_tool_context<T: Any + Send + Sync + 'static>(
        mut self,
        tool_name: impl Into<String>,
        context: T,
    ) -> Self {
        self.tools_context
            .insert(tool_name.into(), Arc::new(context));
        self
    }

    /// Set runtime context for a specific tool from an existing Arc.
    pub fn with_tool_context_arc(
        mut self,
        tool_name: impl Into<String>,
        context: RuntimeContext,
    ) -> Self {
        self.tools_context.insert(tool_name.into(), context);
        self
    }

    /// Set the tool approval collector.
    pub fn with_tool_call_approval(mut self, approval: Arc<dyn ToolApprovalCollector>) -> Self {
        self.tool_call_approval = Some(approval);
        self
    }

    /// Set the telemetry configuration.
    pub fn with_telemetry(mut self, telemetry: crate::telemetry::TelemetrySettings) -> Self {
        self.telemetry = Some(telemetry);
        self
    }
}

/// A part of the text stream.
#[derive(Debug)]
pub enum TextStreamPart {
    /// Text delta.
    TextDelta {
        /// The text delta.
        delta: String,
    },
    /// Reasoning delta.
    ReasoningDelta {
        /// The reasoning delta.
        delta: String,
    },
    /// Tool call started.
    ToolCallStart {
        /// The tool call ID.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
    },
    /// Tool call delta (partial arguments).
    ToolCallDelta {
        /// The tool call ID.
        tool_call_id: String,
        /// The arguments delta.
        delta: String,
    },
    /// Tool call complete.
    ToolCall {
        /// The tool call.
        tool_call: ToolCall,
    },
    /// Tool result from executed tool.
    ToolResult {
        /// The tool result.
        result: ToolResult,
    },
    /// Tool execution error.
    ToolError {
        /// The tool error.
        error: ToolError,
    },
    /// Tool approval has been requested for an upcoming tool execution.
    ///
    /// Consumers should respond with a `ToolApprovalResponse`
    /// (approve / deny) before the tool can run.
    ToolApprovalRequest {
        /// ID of the approval request — referenced by the matching response.
        approval_id: String,
        /// The pending tool call awaiting approval.
        tool_call: ToolCall,
        /// True when the SDK auto-resolved the request (no human prompt).
        is_automatic: bool,
    },
    /// Tool approval response — emitted after a request is resolved.
    ToolApprovalResponse {
        /// ID of the approval request this response is for.
        approval_id: String,
        /// The tool call that was approved or denied.
        tool_call: ToolCall,
        /// Whether the approval was granted (true) or denied (false).
        approved: bool,
        /// Optional reason for approval / denial.
        reason: Option<String>,
        /// Whether the underlying tool is provider-executed.
        /// Only provider-executed approval responses round-trip to the model.
        provider_executed: bool,
    },
    /// Source referenced in the response.
    Source {
        /// The source.
        source: Source,
    },
    /// Generated file.
    File {
        /// The generated file.
        file: GeneratedFile,
    },
    /// Message generation started (emitted at the start of each step).
    MessageStart,
    /// Message generation finished (emitted at the end of each step).
    MessageFinish,
    /// Step finished.
    StepFinish {
        /// The step result.
        step: Box<StepResult>,
    },
    /// Finish event.
    Finish {
        /// The finish reason.
        finish_reason: FinishReason,
        /// Token usage.
        usage: Usage,
        /// The raw finish reason string from the provider.
        raw_finish_reason: Option<String>,
    },
    /// Error occurred.
    Error {
        /// The error.
        error: AIError,
    },
    /// Text content started.
    TextStart {
        /// The text segment ID.
        id: String,
    },
    /// Text content ended.
    TextEnd {
        /// The text segment ID.
        id: String,
    },
    /// Reasoning content started.
    ReasoningStart {
        /// The reasoning segment ID.
        id: String,
    },
    /// Reasoning content ended.
    ReasoningEnd {
        /// The reasoning segment ID.
        id: String,
    },
    /// Tool input streaming started.
    ToolInputStart {
        /// The tool call ID.
        id: String,
        /// The tool name.
        tool_name: String,
    },
    /// Tool input delta (partial arguments).
    ToolInputDelta {
        /// The tool call ID.
        id: String,
        /// The input delta.
        delta: String,
    },
    /// Tool input streaming ended.
    ToolInputEnd {
        /// The tool call ID.
        id: String,
    },
    /// Step started.
    StartStep {
        /// The request metadata (if available).
        request: Option<serde_json::Value>,
        /// Warnings from the provider.
        warnings: Vec<Warning>,
    },
    /// Stream started.
    Start,
    /// Stream aborted.
    Abort {
        /// The reason for the abort.
        reason: String,
    },
    /// Raw provider event.
    Raw {
        /// The raw event value.
        value: serde_json::Value,
    },
}

/// A lazy-evaluated value backed by a oneshot channel.
///
/// The value is produced by the stream processing task and can be
/// awaited after the stream completes.
pub struct Lazy<T> {
    rx: tokio::sync::oneshot::Receiver<T>,
}

impl<T> Lazy<T> {
    /// Create a new lazy value from a oneshot receiver.
    pub fn new(rx: tokio::sync::oneshot::Receiver<T>) -> Self {
        Self { rx }
    }

    /// Await and retrieve the value.
    ///
    /// Returns an error if the stream ended without producing a value
    /// (i.e., the sender was dropped).
    pub async fn get(self) -> Result<T, AIError> {
        self.rx
            .await
            .map_err(|_| AIError::Internal("Stream ended without producing value".to_string()))
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Lazy<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lazy").finish_non_exhaustive()
    }
}

/// Result of `stream_text`.
///
/// This struct provides both streaming access and async methods to get
/// the final result.
pub struct StreamTextResult {
    /// The full stream of text parts (text, reasoning, tool calls, steps, etc.).
    pub stream: Pin<Box<dyn Stream<Item = TextStreamPart> + Send>>,
    /// The model ID.
    pub model_id: String,
    /// Lazily-resolved full text (available after stream completes).
    pub text: Lazy<String>,
    /// Lazily-resolved full reasoning text (available after stream completes).
    pub reasoning: Lazy<String>,
    /// Lazily-resolved tool calls (available after stream completes).
    pub tool_calls_result: Lazy<Vec<ToolCall>>,
    /// Lazily-resolved tool results (available after stream completes).
    pub tool_results_result: Lazy<Vec<ToolResult>>,
    /// Lazily-resolved step results (available after stream completes).
    pub steps_result: Lazy<Vec<StepResult>>,
    /// Lazily-resolved total usage (available after stream completes).
    pub usage_result: Lazy<Usage>,
    /// Lazily-resolved finish reason (available after stream completes).
    pub finish_reason_result: Lazy<FinishReason>,
    /// Lazily-resolved warnings (available after stream completes).
    pub warnings_result: Lazy<Vec<Warning>>,
}

impl StreamTextResult {
    /// Create a new stream text result.
    ///
    /// **Warning**: This simple constructor drops all lazy senders immediately,
    /// so all `Lazy<T>.get()` calls will return errors. Use `stream_text()`
    /// instead for a fully functional result.
    #[doc(hidden)]
    pub fn new(
        stream: Pin<Box<dyn Stream<Item = TextStreamPart> + Send>>,
        model_id: String,
    ) -> Self {
        // Create dummy oneshot channels for the simple constructor
        let (_, text_rx) = tokio::sync::oneshot::channel();
        let (_, reasoning_rx) = tokio::sync::oneshot::channel();
        let (_, tool_calls_rx) = tokio::sync::oneshot::channel();
        let (_, tool_results_rx) = tokio::sync::oneshot::channel();
        let (_, steps_rx) = tokio::sync::oneshot::channel();
        let (_, usage_rx) = tokio::sync::oneshot::channel();
        let (_, finish_reason_rx) = tokio::sync::oneshot::channel();
        let (_, warnings_rx) = tokio::sync::oneshot::channel();

        Self {
            stream,
            model_id,
            text: Lazy::new(text_rx),
            reasoning: Lazy::new(reasoning_rx),
            tool_calls_result: Lazy::new(tool_calls_rx),
            tool_results_result: Lazy::new(tool_results_rx),
            steps_result: Lazy::new(steps_rx),
            usage_result: Lazy::new(usage_rx),
            finish_reason_result: Lazy::new(finish_reason_rx),
            warnings_result: Lazy::new(warnings_rx),
        }
    }

    /// Create a new stream text result with lazy receivers.
    fn with_lazy(
        stream: Pin<Box<dyn Stream<Item = TextStreamPart> + Send>>,
        model_id: String,
        lazy_receivers: LazyReceivers,
    ) -> Self {
        Self {
            stream,
            model_id,
            text: Lazy::new(lazy_receivers.text),
            reasoning: Lazy::new(lazy_receivers.reasoning),
            tool_calls_result: Lazy::new(lazy_receivers.tool_calls),
            tool_results_result: Lazy::new(lazy_receivers.tool_results),
            steps_result: Lazy::new(lazy_receivers.steps),
            usage_result: Lazy::new(lazy_receivers.usage),
            finish_reason_result: Lazy::new(lazy_receivers.finish_reason),
            warnings_result: Lazy::new(lazy_receivers.warnings),
        }
    }

    /// Collect the stream into a final text string.
    pub async fn into_text(mut self) -> Result<String, AIError> {
        let mut text = String::new();
        while let Some(part) = self.stream.next().await {
            match part {
                TextStreamPart::TextDelta { delta } => {
                    text.push_str(&delta);
                }
                TextStreamPart::Error { error } => {
                    return Err(error);
                }
                _ => {}
            }
        }
        Ok(text)
    }

    /// Consume the stream and collect into a full `GenerateTextResult`.
    ///
    /// This processes all stream parts and builds the complete result
    /// with text, reasoning, tool calls, steps, and usage information.
    pub async fn into_result(mut self) -> Result<GenerateTextResult, AIError> {
        let mut text = String::new();
        let mut steps: Vec<StepResult> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_results: Vec<ToolResult> = Vec::new();
        let mut total_usage = Usage::default();
        let mut finish_reason = FinishReason::end_turn();

        while let Some(part) = self.stream.next().await {
            match part {
                TextStreamPart::TextDelta { delta } => {
                    text.push_str(&delta);
                }
                TextStreamPart::ToolCall { tool_call } => {
                    tool_calls.push(tool_call);
                }
                TextStreamPart::ToolResult { result } => {
                    tool_results.push(result);
                }
                TextStreamPart::StepFinish { step } => {
                    steps.push(*step);
                }
                TextStreamPart::Finish {
                    finish_reason: fr,
                    usage,
                    ..
                } => {
                    finish_reason = fr;
                    total_usage = usage;
                }
                TextStreamPart::Error { error } => {
                    return Err(error);
                }
                _ => {}
            }
        }

        let mut result = GenerateTextResult::new(text, total_usage.clone(), finish_reason);
        result.model_id = Some(self.model_id);
        result.total_usage = total_usage;
        result.tool_calls = tool_calls;
        result.tool_results = tool_results;
        result.steps = steps;
        Ok(result)
    }

    /// Create a text-only stream that filters to only text deltas.
    ///
    /// Returns a stream of `String` values, one per text delta.
    pub fn text_stream(self) -> Pin<Box<dyn Stream<Item = String> + Send>> {
        Box::pin(self.stream.filter_map(|part| async move {
            match part {
                TextStreamPart::TextDelta { delta } => Some(delta),
                _ => None,
            }
        }))
    }

    /// Create a reasoning-only stream that filters to only reasoning deltas.
    ///
    /// Returns a stream of `String` values, one per reasoning delta.
    pub fn reasoning_stream(self) -> Pin<Box<dyn Stream<Item = String> + Send>> {
        Box::pin(self.stream.filter_map(|part| async move {
            match part {
                TextStreamPart::ReasoningDelta { delta } => Some(delta),
                _ => None,
            }
        }))
    }
}

/// Receivers for lazy-evaluated values from the stream processing task.
struct LazyReceivers {
    text: tokio::sync::oneshot::Receiver<String>,
    reasoning: tokio::sync::oneshot::Receiver<String>,
    tool_calls: tokio::sync::oneshot::Receiver<Vec<ToolCall>>,
    tool_results: tokio::sync::oneshot::Receiver<Vec<ToolResult>>,
    steps: tokio::sync::oneshot::Receiver<Vec<StepResult>>,
    usage: tokio::sync::oneshot::Receiver<Usage>,
    finish_reason: tokio::sync::oneshot::Receiver<FinishReason>,
    warnings: tokio::sync::oneshot::Receiver<Vec<Warning>>,
}

/// Senders for lazy-evaluated values, used by the stream processing task.
struct LazySenders {
    text: tokio::sync::oneshot::Sender<String>,
    reasoning: tokio::sync::oneshot::Sender<String>,
    tool_calls: tokio::sync::oneshot::Sender<Vec<ToolCall>>,
    tool_results: tokio::sync::oneshot::Sender<Vec<ToolResult>>,
    steps: tokio::sync::oneshot::Sender<Vec<StepResult>>,
    usage: tokio::sync::oneshot::Sender<Usage>,
    finish_reason: tokio::sync::oneshot::Sender<FinishReason>,
    warnings: tokio::sync::oneshot::Sender<Vec<Warning>>,
}

/// Create paired senders and receivers for lazy values.
fn create_lazy_channels() -> (LazySenders, LazyReceivers) {
    let (text_tx, text_rx) = tokio::sync::oneshot::channel();
    let (reasoning_tx, reasoning_rx) = tokio::sync::oneshot::channel();
    let (tool_calls_tx, tool_calls_rx) = tokio::sync::oneshot::channel();
    let (tool_results_tx, tool_results_rx) = tokio::sync::oneshot::channel();
    let (steps_tx, steps_rx) = tokio::sync::oneshot::channel();
    let (usage_tx, usage_rx) = tokio::sync::oneshot::channel();
    let (finish_reason_tx, finish_reason_rx) = tokio::sync::oneshot::channel();
    let (warnings_tx, warnings_rx) = tokio::sync::oneshot::channel();

    let senders = LazySenders {
        text: text_tx,
        reasoning: reasoning_tx,
        tool_calls: tool_calls_tx,
        tool_results: tool_results_tx,
        steps: steps_tx,
        usage: usage_tx,
        finish_reason: finish_reason_tx,
        warnings: warnings_tx,
    };

    let receivers = LazyReceivers {
        text: text_rx,
        reasoning: reasoning_rx,
        tool_calls: tool_calls_rx,
        tool_results: tool_results_rx,
        steps: steps_rx,
        usage: usage_rx,
        finish_reason: finish_reason_rx,
        warnings: warnings_rx,
    };

    (senders, receivers)
}

/// Stream text from a prompt.
///
/// This function streams text generation from a language model.
/// It supports tool calling with automatic tool execution.
///
/// # Arguments
///
/// * `options` - The streaming options including model, prompt, and settings.
///
/// # Returns
///
/// A `StreamTextResult` containing the stream of text parts.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{stream_text, StreamTextOptions, LanguageModel, Prompt};
///
/// let result = stream_text(StreamTextOptions {
///     model: "claude-3-sonnet".into(),
///     prompt: Prompt::user("Hello, world!"),
///     ..Default::default()
/// });
///
/// // Consume the stream
/// while let Some(part) = result.stream.next().await {
///     match part {
///         TextStreamPart::TextDelta { delta } => print!("{}", delta),
///         _ => {}
///     }
/// }
/// ```
#[tracing::instrument(skip_all)]
pub fn stream_text(options: StreamTextOptions) -> StreamTextResult {
    let (tx, rx) = tokio::sync::mpsc::channel(32);

    // Resolve model_id before spawning so we can pass it to the result
    let model_id = crate::model::resolve_language_model_id(&options.model).unwrap_or_default();

    // Create lazy channels
    let (lazy_senders, lazy_receivers) = create_lazy_channels();

    // Extract total timeout before moving options
    let timeout_ms = options.settings.timeout.as_ref().and_then(|t| t.total_ms);

    // Spawn the streaming task
    tokio::spawn(async move {
        let result = if let Some(total_ms) = timeout_ms {
            match tokio::time::timeout(
                std::time::Duration::from_millis(total_ms),
                stream_text_inner(options, tx.clone(), lazy_senders),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    let _ = tx
                        .send(TextStreamPart::Error {
                            error: AIError::Timeout(format!("Stream timed out after {total_ms}ms")),
                        })
                        .await;
                    return;
                }
            }
        } else {
            stream_text_inner(options, tx.clone(), lazy_senders).await
        };
        if let Err(e) = result {
            let _ = tx.send(TextStreamPart::Error { error: e }).await;
        }
    });

    let stream = ReceiverStream::new(rx);
    StreamTextResult::with_lazy(Box::pin(stream), model_id, lazy_receivers)
}

async fn stream_text_inner(
    options: StreamTextOptions,
    tx: tokio::sync::mpsc::Sender<TextStreamPart>,
    lazy_senders: LazySenders,
) -> Result<(), AIError> {
    // Generate unique call ID for this generation session
    let call_id = vercel_ai_provider_utils::generate_id("call");

    // Resolve the model
    let model = resolve_language_model(options.model)?;
    let model_id = model.model_id().to_string();
    let provider_id = model.provider().to_string();

    // Build telemetry integrations
    let integrations = crate::telemetry::build_integrations(options.telemetry.as_ref());

    // Build model info for callbacks
    let model_info = CallbackModelInfo::new(&provider_id, &model_id);

    // Build the initial prompt through standardization pipeline
    let raw_messages = options.prompt.to_model_prompt();
    let mut messages = crate::prompt::convert_to_language_model_prompt(None, raw_messages)
        .map_err(|e| AIError::InvalidArgument(e.to_string()))?;

    // Get tools if available
    let tools = options.tools.as_ref();
    let tool_definitions: Option<Vec<LanguageModelV4Tool>> = tools.map(|t| {
        t.definitions()
            .into_iter()
            .map(|d| LanguageModelV4Tool::function(d.clone()))
            .collect()
    });

    // Build tool names list for events
    let tool_names: Vec<String> = tool_definitions
        .as_ref()
        .map(|defs| defs.iter().map(|d| d.name().to_string()).collect())
        .unwrap_or_default();

    // Call on_start callback + telemetry (after messages and tools are built)
    let mut start_event = OnStartEvent::new(&call_id, model_info.clone())
        .with_messages(messages.clone())
        .with_tools(tool_names.clone())
        .with_settings(&options.settings);
    if let Some(ref provider_opts) = options.provider_options {
        start_event = start_event.with_provider_options(provider_opts.clone());
    }
    if let Some(ref signal) = options.abort_signal {
        start_event = start_event.with_abort_signal(signal.clone());
    }
    if let Some(ref telemetry) = options.telemetry {
        start_event = start_event.with_telemetry(telemetry);
    }
    crate::telemetry::notify_start(
        options.callbacks.on_start.as_deref(),
        &integrations,
        &start_event,
    )
    .await;

    // Emit Start event at the beginning of the stream
    let _ = tx.send(TextStreamPart::Start).await;

    // Track steps
    let max_steps = options.max_steps.unwrap_or(1);
    let mut total_usage = Usage::default();
    let mut steps: Vec<StepResult> = Vec::new();
    let mut runtime_context = options.runtime_context.clone();
    let mut tools_context = options.tools_context.clone();

    // Track collected data for lazy values
    let mut all_text = String::new();
    let mut all_reasoning = String::new();
    let mut all_tool_calls: Vec<ToolCall> = Vec::new();
    let mut all_tool_results: Vec<ToolResult> = Vec::new();
    let mut all_warnings: Vec<Warning> = Vec::new();
    let mut final_finish_reason = FinishReason::end_turn();

    // Build retry config
    let retry_config = options
        .retry_config
        .clone()
        .or_else(|| {
            options
                .settings
                .max_retries
                .map(|max_retries| RetryConfig::new().with_max_retries(max_retries))
        })
        .unwrap_or_default();

    // Multi-step loop
    for step in 0..max_steps {
        // Check for cancellation
        if let Some(ref signal) = options.abort_signal
            && signal.is_cancelled()
        {
            break;
        }

        // Check stop conditions
        if !options.stop_when.is_empty()
            && super::stop_condition::is_stop_condition_met(&options.stop_when, &steps)
        {
            break;
        }

        // Apply prepare_step overrides
        let step_tool_choice;
        let step_active_tools;
        let step_model;
        let step_provider_options;
        let step_messages;
        if let Some(ref prepare) = options.prepare_step {
            let ctx = PrepareStepContext {
                step,
                steps: steps.clone(),
                model_id: model_id.clone(),
                messages: messages.clone(),
                runtime_context: runtime_context.clone(),
                tools_context: tools_context.clone(),
            };
            let overrides = prepare(&ctx);
            step_tool_choice = overrides
                .as_ref()
                .and_then(|o| o.tool_choice.clone())
                .or_else(|| options.tool_choice.clone());
            step_active_tools = overrides
                .as_ref()
                .and_then(|o| o.active_tools.clone())
                .or_else(|| options.active_tools.clone());
            step_model = match overrides.as_ref().and_then(|o| o.model.clone()) {
                Some(m) => Some(resolve_language_model(m)?),
                None => None,
            };
            step_provider_options = overrides.as_ref().and_then(|o| o.provider_options.clone());
            if let Some(ctx) = overrides.as_ref().and_then(|o| o.runtime_context.clone()) {
                runtime_context = Some(ctx);
            }
            if let Some(ctxs) = overrides.as_ref().and_then(|o| o.tools_context.clone()) {
                tools_context = ctxs;
            }
            // Handle messages override, then system prompt override
            if let Some(msgs) = overrides.as_ref().and_then(|o| o.messages.clone()) {
                // Use the overridden messages directly, replacing the entire array
                step_messages = msgs;
            } else if let Some(ref overrides) = overrides
                && let Some(ref sys) = overrides.system
            {
                // If system prompt override, prepend it to messages
                let mut new_messages =
                    vec![vercel_ai_provider::LanguageModelV4Message::system(sys)];
                // Skip any existing system messages at the start
                let non_system = messages
                    .iter()
                    .skip_while(|m| {
                        matches!(m, vercel_ai_provider::LanguageModelV4Message::System { .. })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                new_messages.extend(non_system);
                step_messages = new_messages;
            } else {
                step_messages = messages.clone();
            }
        } else {
            step_tool_choice = options.tool_choice.clone();
            step_active_tools = options.active_tools.clone();
            step_model = None;
            step_provider_options = None;
            step_messages = messages.clone();
        }
        let step_messages_for_tools = step_messages.clone();
        let step_tools_context = tools_context.clone();

        let effective_model = step_model.as_ref().unwrap_or(&model);
        // Deep-merge call-level + step-level provider options.
        // Step keys layer onto call keys; nested objects merge recursively.
        let effective_provider_options = build_call_options::merge_provider_options(
            options.provider_options.as_ref(),
            step_provider_options.as_ref(),
        );

        // Filter active tools
        let effective_tools =
            build_call_options::filter_active_tools(&tool_definitions, &step_active_tools);

        // Build call options using shared builder. abort_signal flows
        // separately to do_stream — not part of CallOptions.
        let call_options = build_call_options::build_call_options(
            &options.settings,
            &step_tool_choice,
            &effective_provider_options,
            &options.output,
            step_messages,
            &effective_tools,
        );

        // Call on_step_start callback + telemetry
        let step_start_event = OnStepStartEvent::new(&call_id, step, model_info.clone());
        crate::telemetry::notify_step_start(
            options.callbacks.on_step_start.as_deref(),
            &integrations,
            &step_start_event,
        )
        .await;

        // Emit StartStep event
        let _ = tx
            .send(TextStreamPart::StartStep {
                request: None,
                warnings: Vec::new(),
            })
            .await;

        // Emit MessageStart
        let _ = tx.send(TextStreamPart::MessageStart).await;

        // Execute with retry for stream initialization
        let stream_result = execute_stream_with_retry(
            effective_model,
            &call_options,
            retry_config.clone(),
            options.abort_signal.clone(),
        )
        .await?;

        // Process the stream
        let mut current_text = String::new();
        let mut current_reasoning = String::new();
        let mut current_tool_id: Option<String> = None;
        let mut current_tool_input = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage = Usage::default();
        let mut finish_reason = FinishReason::end_turn();
        let mut raw_finish_reason: Option<String> = None;
        let mut content: Vec<AssistantContentPart> = Vec::new();
        let mut step_warnings: Vec<Warning> = Vec::new();

        let mut stream = stream_result.stream;

        while let Some(part_result) = stream.next().await {
            match part_result {
                Ok(part) => match part {
                    LanguageModelV4StreamPart::StreamStart { warnings } => {
                        step_warnings = warnings.clone();
                        all_warnings.extend(warnings);
                    }
                    LanguageModelV4StreamPart::TextStart { id, .. } => {
                        current_text.clear();
                        let _ = tx.send(TextStreamPart::TextStart { id }).await;
                    }
                    LanguageModelV4StreamPart::TextDelta { delta, .. } => {
                        current_text.push_str(&delta);
                        all_text.push_str(&delta);
                        // Call on_chunk callback + telemetry
                        crate::telemetry::notify_chunk(
                            options.callbacks.on_chunk.as_deref(),
                            &integrations,
                            &OnChunkEvent::text_delta(&delta),
                        )
                        .await;
                        let _ = tx.send(TextStreamPart::TextDelta { delta }).await;
                    }
                    LanguageModelV4StreamPart::TextEnd { id, .. } => {
                        if !current_text.is_empty() {
                            content.push(AssistantContentPart::text(&current_text));
                        }
                        let _ = tx.send(TextStreamPart::TextEnd { id }).await;
                    }
                    LanguageModelV4StreamPart::ReasoningStart { id, .. } => {
                        current_reasoning.clear();
                        let _ = tx.send(TextStreamPart::ReasoningStart { id }).await;
                    }
                    LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => {
                        current_reasoning.push_str(&delta);
                        all_reasoning.push_str(&delta);
                        // Call on_chunk callback + telemetry
                        crate::telemetry::notify_chunk(
                            options.callbacks.on_chunk.as_deref(),
                            &integrations,
                            &OnChunkEvent::reasoning_delta(&delta),
                        )
                        .await;
                        let _ = tx.send(TextStreamPart::ReasoningDelta { delta }).await;
                    }
                    LanguageModelV4StreamPart::ReasoningEnd { id, .. } => {
                        if !current_reasoning.is_empty() {
                            content.push(AssistantContentPart::reasoning(&current_reasoning));
                        }
                        let _ = tx.send(TextStreamPart::ReasoningEnd { id }).await;
                    }
                    LanguageModelV4StreamPart::ToolInputStart { id, tool_name, .. } => {
                        current_tool_id = Some(id.clone());
                        current_tool_input.clear();

                        // Call on_chunk callback + telemetry
                        crate::telemetry::notify_chunk(
                            options.callbacks.on_chunk.as_deref(),
                            &integrations,
                            &OnChunkEvent::tool_call_start(&id, &tool_name),
                        )
                        .await;

                        // Call on_tool_call_start callback + telemetry
                        let tc_start_event = OnToolCallStartEvent::new(
                            &call_id,
                            step,
                            model_info.clone(),
                            ToolCall::new(&id, &tool_name, serde_json::Value::Null),
                        );
                        crate::telemetry::notify_tool_call_start(
                            options.callbacks.on_tool_call_start.as_deref(),
                            &integrations,
                            &tc_start_event,
                        )
                        .await;

                        // Emit ToolInputStart
                        let _ = tx
                            .send(TextStreamPart::ToolInputStart {
                                id: id.clone(),
                                tool_name: tool_name.clone(),
                            })
                            .await;

                        let _ = tx
                            .send(TextStreamPart::ToolCallStart {
                                tool_call_id: id,
                                tool_name,
                            })
                            .await;
                    }
                    LanguageModelV4StreamPart::ToolInputDelta { id, delta, .. } => {
                        current_tool_input.push_str(&delta);
                        if let Some(tool_id) = current_tool_id.as_ref() {
                            // Call on_chunk callback + telemetry
                            crate::telemetry::notify_chunk(
                                options.callbacks.on_chunk.as_deref(),
                                &integrations,
                                &OnChunkEvent::tool_call_delta(tool_id, &delta),
                            )
                            .await;

                            // Emit ToolInputDelta
                            let _ = tx
                                .send(TextStreamPart::ToolInputDelta {
                                    id: id.clone(),
                                    delta: delta.clone(),
                                })
                                .await;

                            let _ = tx
                                .send(TextStreamPart::ToolCallDelta {
                                    tool_call_id: tool_id.clone(),
                                    delta,
                                })
                                .await;
                        }
                    }
                    LanguageModelV4StreamPart::ToolInputEnd { id, .. } => {
                        // Emit ToolInputEnd
                        let _ = tx.send(TextStreamPart::ToolInputEnd { id }).await;
                    }
                    LanguageModelV4StreamPart::ToolCall(tc) => {
                        let input =
                            serde_json::from_str(&tc.input).unwrap_or(serde_json::Value::Null);
                        let tool_call =
                            ToolCall::new(tc.tool_call_id.clone(), tc.tool_name.clone(), input);
                        tool_calls.push(tool_call.clone());

                        // Send the tool call event
                        let _ = tx.send(TextStreamPart::ToolCall { tool_call }).await;

                        current_tool_id = None;
                    }
                    LanguageModelV4StreamPart::Finish {
                        finish_reason: fr,
                        usage: u,
                        ..
                    } => {
                        raw_finish_reason = fr.raw.clone();
                        finish_reason = fr;
                        usage = u.clone();
                        total_usage.add(&u);
                    }
                    LanguageModelV4StreamPart::Raw { raw_value } => {
                        let _ = tx.send(TextStreamPart::Raw { value: raw_value }).await;
                    }
                    _ => {}
                },
                Err(e) => {
                    let err = AIError::ProviderError(e);
                    // Call on_error callback + telemetry
                    if let Some(ref cb) = options.callbacks.on_error {
                        cb(&err);
                    }
                    crate::telemetry::notify_error(&integrations, &err).await;
                    let _ = tx.send(TextStreamPart::Error { error: err }).await;
                    // Send lazy values before returning
                    send_lazy_values(
                        lazy_senders,
                        all_text,
                        all_reasoning,
                        all_tool_calls,
                        all_tool_results,
                        steps,
                        total_usage,
                        final_finish_reason,
                        all_warnings,
                    );
                    return Ok(());
                }
            }
        }

        // Emit MessageFinish
        let _ = tx.send(TextStreamPart::MessageFinish).await;

        // Check if we need to execute tools
        if !tool_calls.is_empty()
            && let Some(tools_reg) = tools
        {
            // Collect tool approvals if configured
            if let Some(ref approval_collector) = options.tool_call_approval {
                let requests: Vec<ToolApprovalRequest> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        tools_reg.get(&tc.tool_name).map(|tool| {
                            let desc = tool.definition().description.clone();
                            ToolApprovalRequest::new(tc.clone())
                                .with_description(desc.unwrap_or_default())
                        })
                    })
                    .collect();

                if !requests.is_empty()
                    && let Ok(approvals) = approval_collector.collect_approvals(requests).await
                {
                    tool_calls = apply_approvals(tool_calls, &approvals);
                }
            }

            // Execute tool calls concurrently (matching generate_text's join_all pattern)
            let tool_futures: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    let mut exec_options = ToolExecutionOptions::new(&tc.tool_call_id)
                        .with_messages(step_messages_for_tools.clone());
                    if let Some(context) = step_tools_context.get(&tc.tool_name) {
                        exec_options = exec_options.with_context_arc(context.clone());
                    }
                    let tc = tc.clone();
                    async move {
                        let start_time = std::time::Instant::now();
                        let result = super::execute_tool_call::execute_tool_call(
                            &tc,
                            tools_reg,
                            exec_options,
                        )
                        .await;
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        (tc, result, duration_ms)
                    }
                })
                .collect();

            let tool_outcomes = futures::future::join_all(tool_futures).await;

            let mut tool_results = Vec::new();
            for (tc, result, duration_ms) in tool_outcomes {
                let tool_result = match result {
                    Ok(output) => {
                        // Call on_tool_call_finish callback + telemetry (success)
                        let tc_finish_event = OnToolCallFinishEvent::success(
                            &call_id,
                            step,
                            model_info.clone(),
                            tc.clone(),
                            output.clone(),
                            duration_ms,
                        );
                        crate::telemetry::notify_tool_call_finish(
                            options.callbacks.on_tool_call_finish.as_deref(),
                            &integrations,
                            &tc_finish_event,
                        )
                        .await;

                        ToolResult::new(&tc.tool_call_id, &tc.tool_name, output)
                    }
                    Err(e) => {
                        // Emit tool error
                        let _ = tx
                            .send(TextStreamPart::ToolError { error: e.clone() })
                            .await;

                        // Call on_tool_call_finish callback + telemetry (error)
                        let tc_finish_event = OnToolCallFinishEvent::error(
                            &call_id,
                            step,
                            model_info.clone(),
                            tc.clone(),
                            e.to_string(),
                            duration_ms,
                        );
                        crate::telemetry::notify_tool_call_finish(
                            options.callbacks.on_tool_call_finish.as_deref(),
                            &integrations,
                            &tc_finish_event,
                        )
                        .await;

                        ToolResult::error(&tc.tool_call_id, &tc.tool_name, e.to_string())
                    }
                };

                // Send tool result event
                let _ = tx
                    .send(TextStreamPart::ToolResult {
                        result: tool_result.clone(),
                    })
                    .await;

                tool_results.push(tool_result);
            }

            // Collect tool calls and results for lazy values
            all_tool_calls.extend(tool_calls.iter().cloned());
            all_tool_results.extend(tool_results.iter().cloned());

            // Create step result
            let mut step_result = StepResult::new(
                step,
                content_utils::extract_text(&content),
                usage.clone(),
                finish_reason.clone(),
            )
            .with_call_id(&call_id)
            .with_model(CallbackModelInfo::new(&provider_id, &model_id))
            .with_content(content.clone())
            .with_tool_calls(tool_calls.clone())
            .with_tool_results(tool_results.clone())
            .with_warnings(step_warnings);

            // Set raw finish reason
            if let Some(ref raw) = raw_finish_reason {
                step_result = step_result.with_raw_finish_reason(raw);
            }

            final_finish_reason = finish_reason.clone();

            // Call step finish callback + telemetry
            crate::telemetry::notify_step_finish(
                options.callbacks.on_step_finish.as_deref(),
                &integrations,
                &step_result,
            )
            .await;

            // Send step finish event
            let _ = tx
                .send(TextStreamPart::StepFinish {
                    step: Box::new(step_result.clone()),
                })
                .await;

            steps.push(step_result);

            // Add assistant message and tool results to conversation
            messages.push(vercel_ai_provider::LanguageModelV4Message::assistant(
                content,
            ));

            // Add tool results as tool messages using shared utility
            let tool_result_msg = build_tool_result_message(&tool_results);
            messages.push(tool_result_msg);

            // Continue to next step
            continue;
        }

        // No tool calls - this is the final step
        let mut step_result = StepResult::new(
            step,
            content_utils::extract_text(&content),
            usage,
            finish_reason.clone(),
        )
        .with_call_id(&call_id)
        .with_model(CallbackModelInfo::new(&provider_id, &model_id))
        .with_content(content)
        .with_warnings(step_warnings);

        // Set raw finish reason
        if let Some(ref raw) = raw_finish_reason {
            step_result = step_result.with_raw_finish_reason(raw);
        }

        final_finish_reason = finish_reason.clone();

        // Call step finish callback + telemetry
        crate::telemetry::notify_step_finish(
            options.callbacks.on_step_finish.as_deref(),
            &integrations,
            &step_result,
        )
        .await;

        let _ = tx
            .send(TextStreamPart::StepFinish {
                step: Box::new(step_result.clone()),
            })
            .await;

        steps.push(step_result);

        // Call on_finish callback + telemetry
        let finish_step = steps.last().cloned().unwrap_or_default();
        let finish_event = OnFinishEvent::new(finish_step, steps.clone(), total_usage.clone());
        crate::telemetry::notify_finish(
            options.callbacks.on_finish.as_deref(),
            &integrations,
            &finish_event,
        )
        .await;

        // Send finish event
        let _ = tx
            .send(TextStreamPart::Finish {
                finish_reason,
                usage: total_usage.clone(),
                raw_finish_reason: raw_finish_reason.clone(),
            })
            .await;

        // Send lazy values
        send_lazy_values(
            lazy_senders,
            all_text,
            all_reasoning,
            all_tool_calls,
            all_tool_results,
            steps,
            total_usage,
            final_finish_reason,
            all_warnings,
        );

        return Ok(());
    }

    // Reached max steps - send finish event
    let finish_step = steps.last().cloned().unwrap_or_default();
    let finish_event = OnFinishEvent::new(finish_step, steps.clone(), total_usage.clone());
    crate::telemetry::notify_finish(
        options.callbacks.on_finish.as_deref(),
        &integrations,
        &finish_event,
    )
    .await;

    let _ = tx
        .send(TextStreamPart::Finish {
            finish_reason: FinishReason::end_turn(),
            usage: total_usage.clone(),
            raw_finish_reason: None,
        })
        .await;

    // Send lazy values
    send_lazy_values(
        lazy_senders,
        all_text,
        all_reasoning,
        all_tool_calls,
        all_tool_results,
        steps,
        total_usage,
        final_finish_reason,
        all_warnings,
    );

    Ok(())
}

/// Send all collected values through oneshot channels for lazy evaluation.
#[allow(clippy::too_many_arguments)]
fn send_lazy_values(
    senders: LazySenders,
    text: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    tool_results: Vec<ToolResult>,
    steps: Vec<StepResult>,
    usage: Usage,
    finish_reason: FinishReason,
    warnings: Vec<Warning>,
) {
    let _ = senders.text.send(text);
    let _ = senders.reasoning.send(reasoning);
    let _ = senders.tool_calls.send(tool_calls);
    let _ = senders.tool_results.send(tool_results);
    let _ = senders.steps.send(steps);
    let _ = senders.usage.send(usage);
    let _ = senders.finish_reason.send(finish_reason);
    let _ = senders.warnings.send(warnings);
}

/// Execute a streaming request with retry logic.
///
/// `call_options` is borrowed across attempts — no per-attempt clone of
/// the prompt vector. `abort_signal` is `Arc`-backed (cheap refcount bump).
async fn execute_stream_with_retry(
    model: &Arc<dyn vercel_ai_provider::LanguageModelV4>,
    call_options: &LanguageModelV4CallOptions,
    retry_config: RetryConfig,
    abort_signal: Option<CancellationToken>,
) -> Result<vercel_ai_provider::LanguageModelV4StreamResult, AIError> {
    use crate::util::retry::with_retry;

    let provider_name = model.provider().to_string();
    let model_id_str = model.model_id().to_string();

    with_retry(retry_config, abort_signal.clone(), || {
        let model = model.clone();
        let provider_name = provider_name.clone();
        let model_id_str = model_id_str.clone();
        let abort_signal = abort_signal.clone();
        async move {
            model
                .do_stream(call_options, abort_signal)
                .await
                .map_err(|e| crate::prompt::wrap_gateway_error(e, &provider_name, &model_id_str))
        }
    })
    .await
}

#[cfg(test)]
#[path = "stream_text.test.rs"]
mod tests;
