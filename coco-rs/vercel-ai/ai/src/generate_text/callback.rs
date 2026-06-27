//! Callback types for generate_text and stream_text.
//!
//! This module provides callback types that allow users to hook into
//! the generation lifecycle. Event types are shared between user callbacks
//! and telemetry integrations.
//!
//! # Relationship to CoreEvent
//!
//! These callbacks (`OnStartEvent`, `OnStepFinishEvent`, `OnFinishEvent`,
//! `OnErrorEvent`) fire at the **provider boundary** — one abstraction
//! level below `AgentStreamEvent` in the `CoreEvent` envelope.
//!
//! They are **intentionally NOT bridged into `CoreEvent`**
//! (confirmed April 2026, event-system-design.md §1.7, plan WS-9):
//!
//! - Bridging would duplicate data already emitted by the agent loop
//!   (`AgentStreamEvent::TextDelta`, `TurnCompleted.usage`, etc.)
//! - Correct layering: `QueryEngine` listens to these callbacks
//!   internally and translates results into `CoreEvent::Protocol` /
//!   `CoreEvent::Stream` emissions. The callbacks are an
//!   implementation detail of inference, not a public event surface.
//! - Trace correlation between callback telemetry and `CoreEvent`
//!   streams uses shared `session_id` / `turn_id` context, not a
//!   data-flow bridge.
//!
//! For the public event surface visible to SDK consumers, see
//! `coco_types::CoreEvent` and `coco_types::ServerNotification`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::Usage;

use super::generate_text_result::ToolCall;
use super::generate_text_result::ToolResult;
use super::step_result::StepResult;

use vercel_ai_provider::ReasoningLevel;

use crate::types::ProviderOptions;

/// Model information for callback events.
///
/// Combines provider name and model ID into a single struct.
#[derive(Debug, Clone)]
pub struct CallbackModelInfo {
    /// The provider name (e.g., "anthropic", "openai").
    pub provider: String,
    /// The model ID (e.g., "claude-3-sonnet").
    pub model_id: String,
}

impl CallbackModelInfo {
    /// Create new model info.
    pub fn new(provider: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }
}

/// Event data for the on_start callback.
///
/// Contains comprehensive information about the generation request.
#[derive(Clone)]
pub struct OnStartEvent {
    // --- Identity ---
    /// Unique call ID for this generation session.
    pub call_id: String,
    /// The provider name (e.g., "anthropic", "openai").
    pub provider: String,
    /// The model ID (e.g., "claude-3-sonnet").
    pub model_id: String,
    /// Model information (deprecated — use `provider` and `model_id` directly).
    pub model: CallbackModelInfo,

    // --- Prompt ---
    /// System prompt (if any).
    pub system: Option<String>,
    /// The messages being sent to the model.
    pub messages: Vec<LanguageModelV4Message>,

    // --- Tools ---
    /// Tool names available.
    pub tools: Vec<String>,
    /// Tool choice configuration description.
    pub tool_choice: Option<String>,
    /// Active tools filter.
    pub active_tools: Option<Vec<String>>,

    // --- Call Settings ---
    /// Maximum output tokens.
    pub max_tokens: Option<u64>,
    /// Temperature setting.
    pub temperature: Option<f32>,
    /// Top-p (nucleus) sampling.
    pub top_p: Option<f32>,
    /// Top-k sampling.
    pub top_k: Option<u64>,
    /// Presence penalty.
    pub presence_penalty: Option<f32>,
    /// Frequency penalty.
    pub frequency_penalty: Option<f32>,
    /// Stop sequences.
    pub stop_sequences: Option<Vec<String>>,
    /// Random seed.
    pub seed: Option<u64>,
    /// Provider-agnostic reasoning effort level.
    pub reasoning: Option<ReasoningLevel>,

    // --- Retry / Timeout ---
    /// Maximum retries.
    pub max_retries: Option<u32>,
    /// Custom headers.
    pub headers: Option<HashMap<String, String>>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,

    // --- Cancellation ---
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,

    // --- Telemetry ---
    /// Whether telemetry is enabled.
    pub is_enabled: Option<bool>,
    /// Whether to record inputs.
    pub record_inputs: Option<bool>,
    /// Whether to record outputs.
    pub record_outputs: Option<bool>,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl std::fmt::Debug for OnStartEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnStartEvent")
            .field("call_id", &self.call_id)
            .field("provider", &self.provider)
            .field("model_id", &self.model_id)
            .field("system", &self.system)
            .field("messages_count", &self.messages.len())
            .field("tools", &self.tools)
            .field("tool_choice", &self.tool_choice)
            .field("reasoning", &self.reasoning)
            .field("function_id", &self.function_id)
            .finish()
    }
}

impl OnStartEvent {
    /// Create a new on_start event with minimal required fields.
    pub fn new(call_id: impl Into<String>, model: CallbackModelInfo) -> Self {
        let provider = model.provider.clone();
        let model_id = model.model_id.clone();
        Self {
            call_id: call_id.into(),
            provider,
            model_id,
            model,
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            active_tools: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            presence_penalty: None,
            frequency_penalty: None,
            stop_sequences: None,
            seed: None,
            reasoning: None,
            max_retries: None,
            headers: None,
            provider_options: None,
            abort_signal: None,
            is_enabled: None,
            record_inputs: None,
            record_outputs: None,
            function_id: None,
            metadata: None,
        }
    }

    /// Set the system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: Vec<LanguageModelV4Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the tool names.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the tool choice description.
    pub fn with_tool_choice(mut self, tool_choice: impl Into<String>) -> Self {
        self.tool_choice = Some(tool_choice.into());
        self
    }

    /// Set the active tools filter.
    pub fn with_active_tools(mut self, active_tools: Vec<String>) -> Self {
        self.active_tools = Some(active_tools);
        self
    }

    /// Populate call settings fields.
    pub fn with_settings(mut self, settings: &crate::prompt::CallSettings) -> Self {
        self.max_tokens = settings.max_tokens;
        self.temperature = settings.temperature;
        self.top_p = settings.top_p;
        self.top_k = settings.top_k;
        self.presence_penalty = settings.presence_penalty;
        self.frequency_penalty = settings.frequency_penalty;
        self.stop_sequences = settings.stop_sequences.clone();
        self.seed = settings.seed;
        self.reasoning = settings.reasoning;
        self.max_retries = settings.max_retries;
        if let Some(ref h) = settings.headers {
            self.headers = Some(h.clone());
        }
        self
    }

    /// Set custom headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Populate telemetry fields from settings.
    pub fn with_telemetry(mut self, telemetry: &crate::telemetry::TelemetrySettings) -> Self {
        self.is_enabled = telemetry.is_enabled;
        self.record_inputs = telemetry.record_inputs;
        self.record_outputs = telemetry.record_outputs;
        self.function_id = telemetry.function_id.clone();
        self.metadata = telemetry.metadata.as_ref().and_then(|m| {
            if let serde_json::Value::Object(map) = m {
                Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            } else {
                None
            }
        });
        self
    }
}

/// Event data for the on_step_start callback.
///
/// Contains step context and model configuration for the current step.
#[derive(Clone)]
pub struct OnStepStartEvent {
    /// Unique call ID.
    pub call_id: String,
    /// The step number (0-indexed).
    pub step_number: u32,
    /// The provider name.
    pub provider: String,
    /// The model ID.
    pub model_id: String,
    /// Model information (deprecated — use `provider` and `model_id` directly).
    pub model: CallbackModelInfo,
    /// System prompt (if any).
    pub system: Option<String>,
    /// Current messages.
    pub messages: Vec<LanguageModelV4Message>,
    /// Tool names available.
    pub tools: Vec<String>,
    /// Tool choice configuration description.
    pub tool_choice: Option<String>,
    /// Active tools filter.
    pub active_tools: Option<Vec<String>>,
    /// Prior steps completed.
    pub steps: Vec<StepResult>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal.
    pub abort_signal: Option<CancellationToken>,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl std::fmt::Debug for OnStepStartEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnStepStartEvent")
            .field("call_id", &self.call_id)
            .field("step_number", &self.step_number)
            .field("provider", &self.provider)
            .field("model_id", &self.model_id)
            .field("messages_count", &self.messages.len())
            .field("tools", &self.tools)
            .field("steps_count", &self.steps.len())
            .finish()
    }
}

impl OnStepStartEvent {
    /// Create a new on_step_start event.
    pub fn new(call_id: impl Into<String>, step_number: u32, model: CallbackModelInfo) -> Self {
        let provider = model.provider.clone();
        let model_id = model.model_id.clone();
        Self {
            call_id: call_id.into(),
            step_number,
            provider,
            model_id,
            model,
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            active_tools: None,
            steps: Vec::new(),
            provider_options: None,
            abort_signal: None,
            function_id: None,
            metadata: None,
        }
    }

    /// Set the system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: Vec<LanguageModelV4Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the tool names.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the tool choice.
    pub fn with_tool_choice(mut self, tool_choice: impl Into<String>) -> Self {
        self.tool_choice = Some(tool_choice.into());
        self
    }

    /// Set the active tools filter.
    pub fn with_active_tools(mut self, active_tools: Vec<String>) -> Self {
        self.active_tools = Some(active_tools);
        self
    }

    /// Set the prior steps.
    pub fn with_steps(mut self, steps: Vec<StepResult>) -> Self {
        self.steps = steps;
        self
    }

    /// Set the provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the telemetry function ID.
    pub fn with_function_id(mut self, id: impl Into<String>) -> Self {
        self.function_id = Some(id.into());
        self
    }

    /// Set the telemetry metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Event for tool call start.
#[derive(Clone)]
pub struct OnToolCallStartEvent {
    /// Unique call ID.
    pub call_id: String,
    /// The step number.
    pub step_number: u32,
    /// Model information.
    pub model: CallbackModelInfo,
    /// The tool call being started.
    pub tool_call: ToolCall,
    /// Current messages.
    pub messages: Vec<LanguageModelV4Message>,
    /// Abort signal.
    pub abort_signal: Option<CancellationToken>,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl std::fmt::Debug for OnToolCallStartEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnToolCallStartEvent")
            .field("call_id", &self.call_id)
            .field("step_number", &self.step_number)
            .field("model", &self.model)
            .field("tool_call", &self.tool_call)
            .finish()
    }
}

impl OnToolCallStartEvent {
    /// Create a new tool call start event.
    pub fn new(
        call_id: impl Into<String>,
        step_number: u32,
        model: CallbackModelInfo,
        tool_call: ToolCall,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            step_number,
            model,
            tool_call,
            messages: Vec::new(),
            abort_signal: None,
            function_id: None,
            metadata: None,
        }
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: Vec<LanguageModelV4Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the telemetry function ID.
    pub fn with_function_id(mut self, id: impl Into<String>) -> Self {
        self.function_id = Some(id.into());
        self
    }

    /// Set the telemetry metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Outcome of a tool call execution.
#[derive(Debug, Clone)]
pub enum ToolCallOutcome {
    /// Tool call succeeded.
    Success {
        /// The output value.
        output: serde_json::Value,
    },
    /// Tool call failed.
    Error {
        /// The error message.
        error: String,
    },
}

/// Event for tool call finish.
///
/// Uses a discriminated union (`ToolCallOutcome`) for success/error.
#[derive(Clone)]
pub struct OnToolCallFinishEvent {
    /// Unique call ID.
    pub call_id: String,
    /// The step number.
    pub step_number: u32,
    /// Model information.
    pub model: CallbackModelInfo,
    /// The tool call that finished.
    pub tool_call: ToolCall,
    /// Current messages.
    pub messages: Vec<LanguageModelV4Message>,
    /// Abort signal.
    pub abort_signal: Option<CancellationToken>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// The outcome (success or error).
    pub outcome: ToolCallOutcome,
}

impl std::fmt::Debug for OnToolCallFinishEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnToolCallFinishEvent")
            .field("call_id", &self.call_id)
            .field("step_number", &self.step_number)
            .field("model", &self.model)
            .field("tool_call", &self.tool_call)
            .field("duration_ms", &self.duration_ms)
            .field("outcome", &self.outcome)
            .finish()
    }
}

impl OnToolCallFinishEvent {
    /// Create a new tool call finish event with a success outcome.
    pub fn success(
        call_id: impl Into<String>,
        step_number: u32,
        model: CallbackModelInfo,
        tool_call: ToolCall,
        output: serde_json::Value,
        duration_ms: u64,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            step_number,
            model,
            tool_call,
            messages: Vec::new(),
            abort_signal: None,
            duration_ms,
            function_id: None,
            metadata: None,
            outcome: ToolCallOutcome::Success { output },
        }
    }

    /// Create a new tool call finish event with an error outcome.
    pub fn error(
        call_id: impl Into<String>,
        step_number: u32,
        model: CallbackModelInfo,
        tool_call: ToolCall,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            step_number,
            model,
            tool_call,
            messages: Vec::new(),
            abort_signal: None,
            duration_ms,
            function_id: None,
            metadata: None,
            outcome: ToolCallOutcome::Error {
                error: error.into(),
            },
        }
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: Vec<LanguageModelV4Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the telemetry function ID.
    pub fn with_function_id(mut self, id: impl Into<String>) -> Self {
        self.function_id = Some(id.into());
        self
    }

    /// Set the telemetry metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Check if the outcome is an error.
    pub fn is_error(&self) -> bool {
        matches!(self.outcome, ToolCallOutcome::Error { .. })
    }
}

/// `OnStepFinishEvent` IS a `StepResult` — step finish events carry the full step result.
pub type OnStepFinishEvent = StepResult;

/// Event data for the on_finish callback.
///
/// Wraps the final StepResult and includes aggregate data.
#[derive(Debug, Clone)]
pub struct OnFinishEvent {
    /// The final step result (includes all StepResult fields).
    pub step_result: StepResult,
    /// All steps taken during generation.
    pub steps: Vec<StepResult>,
    /// Total usage across all steps.
    pub total_usage: Usage,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl OnFinishEvent {
    /// Create a new on_finish event.
    pub fn new(step_result: StepResult, steps: Vec<StepResult>, total_usage: Usage) -> Self {
        Self {
            step_result,
            steps,
            total_usage,
            function_id: None,
            metadata: None,
        }
    }

    /// Set the telemetry function ID.
    pub fn with_function_id(mut self, id: impl Into<String>) -> Self {
        self.function_id = Some(id.into());
        self
    }

    /// Set the telemetry metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Convenience: get the finish reason from the step result.
    pub fn finish_reason(&self) -> &FinishReason {
        &self.step_result.finish_reason
    }

    /// Convenience: get the text from the step result.
    pub fn text(&self) -> &str {
        &self.step_result.text
    }

    /// Convenience: get the model info.
    pub fn model(&self) -> &CallbackModelInfo {
        &self.step_result.model
    }
}

/// Chunk event data variants for streaming.
#[derive(Debug, Clone)]
pub enum ChunkEventData {
    /// Text delta chunk.
    TextDelta { delta: String },
    /// Reasoning delta chunk.
    ReasoningDelta { delta: String },
    /// Source reference.
    Source(vercel_ai_provider::Source),
    /// Complete tool call.
    ToolCall(ToolCall),
    /// Tool input streaming started.
    ToolInputStart {
        tool_call_id: String,
        tool_name: String,
    },
    /// Tool input delta.
    ToolInputDelta { tool_call_id: String, delta: String },
    /// Tool result.
    ToolResult(ToolResult),
    /// Custom provider-specific content.
    Custom {
        kind: String,
        provider_metadata: Option<vercel_ai_provider::ProviderMetadata>,
    },
    /// Reasoning file content.
    ReasoningFile {
        file: super::generated_file::GeneratedFile,
        provider_metadata: Option<vercel_ai_provider::ProviderMetadata>,
    },
    /// Stream lifecycle event.
    StreamLifecycle {
        event_type: String,
        call_id: String,
        step_number: u32,
    },
}

/// Event for on_chunk callback in stream_text.
///
/// Uses a typed enum for the chunk data.
#[derive(Debug, Clone)]
pub struct OnChunkEvent {
    /// The chunk data.
    pub chunk: ChunkEventData,
}

impl OnChunkEvent {
    /// Create a text delta chunk event.
    pub fn text_delta(delta: impl Into<String>) -> Self {
        Self {
            chunk: ChunkEventData::TextDelta {
                delta: delta.into(),
            },
        }
    }

    /// Create a reasoning delta chunk event.
    pub fn reasoning_delta(delta: impl Into<String>) -> Self {
        Self {
            chunk: ChunkEventData::ReasoningDelta {
                delta: delta.into(),
            },
        }
    }

    /// Create a tool call start chunk event.
    pub fn tool_call_start(tool_call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            chunk: ChunkEventData::ToolInputStart {
                tool_call_id: tool_call_id.into(),
                tool_name: tool_name.into(),
            },
        }
    }

    /// Create a tool call delta chunk event.
    pub fn tool_call_delta(tool_call_id: impl Into<String>, delta: impl Into<String>) -> Self {
        Self {
            chunk: ChunkEventData::ToolInputDelta {
                tool_call_id: tool_call_id.into(),
                delta: delta.into(),
            },
        }
    }

    /// Create a tool call chunk event.
    pub fn tool_call(tool_call: ToolCall) -> Self {
        Self {
            chunk: ChunkEventData::ToolCall(tool_call),
        }
    }

    /// Create a tool result chunk event.
    pub fn tool_result(result: ToolResult) -> Self {
        Self {
            chunk: ChunkEventData::ToolResult(result),
        }
    }

    /// Create a source chunk event.
    pub fn source(source: vercel_ai_provider::Source) -> Self {
        Self {
            chunk: ChunkEventData::Source(source),
        }
    }

    /// Create a custom content chunk event.
    pub fn custom(
        kind: impl Into<String>,
        provider_metadata: Option<vercel_ai_provider::ProviderMetadata>,
    ) -> Self {
        Self {
            chunk: ChunkEventData::Custom {
                kind: kind.into(),
                provider_metadata,
            },
        }
    }

    /// Create a reasoning file chunk event.
    pub fn reasoning_file(
        file: super::generated_file::GeneratedFile,
        provider_metadata: Option<vercel_ai_provider::ProviderMetadata>,
    ) -> Self {
        Self {
            chunk: ChunkEventData::ReasoningFile {
                file,
                provider_metadata,
            },
        }
    }
}

/// Callbacks for generate_text.
#[derive(Default)]
pub struct GenerateTextCallbacks {
    /// Called when generation starts.
    pub on_start: Option<Arc<dyn Fn(OnStartEvent) + Send + Sync>>,
    /// Called when generation finishes.
    pub on_finish: Option<Arc<dyn Fn(OnFinishEvent) + Send + Sync>>,
    /// Called when a step starts.
    pub on_step_start: Option<Arc<dyn Fn(OnStepStartEvent) + Send + Sync>>,
    /// Called when a step finishes. Event IS a StepResult.
    pub on_step_finish: Option<Arc<dyn Fn(OnStepFinishEvent) + Send + Sync>>,
    /// Called when a tool call starts.
    pub on_tool_call_start: Option<Arc<dyn Fn(OnToolCallStartEvent) + Send + Sync>>,
    /// Called when a tool call finishes.
    pub on_tool_call_finish: Option<Arc<dyn Fn(OnToolCallFinishEvent) + Send + Sync>>,
}

impl GenerateTextCallbacks {
    /// Create new callbacks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the on_start callback.
    pub fn with_on_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStartEvent) + Send + Sync + 'static,
    {
        self.on_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_finish callback.
    pub fn with_on_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnFinishEvent) + Send + Sync + 'static,
    {
        self.on_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_step_start callback.
    pub fn with_on_step_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStepStartEvent) + Send + Sync + 'static,
    {
        self.on_step_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_step_finish callback.
    pub fn with_on_step_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStepFinishEvent) + Send + Sync + 'static,
    {
        self.on_step_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_tool_call_start callback.
    pub fn with_on_tool_call_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnToolCallStartEvent) + Send + Sync + 'static,
    {
        self.on_tool_call_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_tool_call_finish callback.
    pub fn with_on_tool_call_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnToolCallFinishEvent) + Send + Sync + 'static,
    {
        self.on_tool_call_finish = Some(Arc::new(callback));
        self
    }
}

/// Callbacks for stream_text.
#[derive(Default)]
pub struct StreamTextCallbacks {
    /// Called when streaming starts.
    pub on_start: Option<Arc<dyn Fn(OnStartEvent) + Send + Sync>>,
    /// Called when streaming finishes.
    pub on_finish: Option<Arc<dyn Fn(OnFinishEvent) + Send + Sync>>,
    /// Called when a step starts.
    pub on_step_start: Option<Arc<dyn Fn(OnStepStartEvent) + Send + Sync>>,
    /// Called when a step finishes. Event IS a StepResult.
    pub on_step_finish: Option<Arc<dyn Fn(OnStepFinishEvent) + Send + Sync>>,
    /// Called when a chunk is received from the stream.
    pub on_chunk: Option<Arc<dyn Fn(OnChunkEvent) + Send + Sync>>,
    /// Called when a tool call starts.
    pub on_tool_call_start: Option<Arc<dyn Fn(OnToolCallStartEvent) + Send + Sync>>,
    /// Called when a tool call finishes.
    pub on_tool_call_finish: Option<Arc<dyn Fn(OnToolCallFinishEvent) + Send + Sync>>,
    /// Called when an error occurs during streaming.
    #[allow(clippy::type_complexity)]
    pub on_error: Option<Arc<dyn Fn(&crate::error::AIError) + Send + Sync>>,
}

impl StreamTextCallbacks {
    /// Create new callbacks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the on_start callback.
    pub fn with_on_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStartEvent) + Send + Sync + 'static,
    {
        self.on_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_finish callback.
    pub fn with_on_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnFinishEvent) + Send + Sync + 'static,
    {
        self.on_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_step_start callback.
    pub fn with_on_step_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStepStartEvent) + Send + Sync + 'static,
    {
        self.on_step_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_step_finish callback.
    pub fn with_on_step_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnStepFinishEvent) + Send + Sync + 'static,
    {
        self.on_step_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_chunk callback.
    pub fn with_on_chunk<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnChunkEvent) + Send + Sync + 'static,
    {
        self.on_chunk = Some(Arc::new(callback));
        self
    }

    /// Set the on_tool_call_start callback.
    pub fn with_on_tool_call_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnToolCallStartEvent) + Send + Sync + 'static,
    {
        self.on_tool_call_start = Some(Arc::new(callback));
        self
    }

    /// Set the on_tool_call_finish callback.
    pub fn with_on_tool_call_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(OnToolCallFinishEvent) + Send + Sync + 'static,
    {
        self.on_tool_call_finish = Some(Arc::new(callback));
        self
    }

    /// Set the on_error callback.
    pub fn with_on_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(&crate::error::AIError) + Send + Sync + 'static,
    {
        self.on_error = Some(Arc::new(callback));
        self
    }
}

#[cfg(test)]
#[path = "callback.test.rs"]
mod tests;
