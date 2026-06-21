//! Generate text from a prompt.
//!
//! This module provides the `generate_text` function for generating text
//! from a language model without streaming.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;

use crate::error::AIError;
use crate::model::LanguageModel;
use crate::model::resolve_language_model;
use crate::prompt::CallSettings;
use crate::prompt::TimeoutConfiguration;
use crate::types::ProviderOptions;
use crate::types::ToolExecutionOptions;
use crate::types::ToolRegistry;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::build_call_options;
use super::callback::CallbackModelInfo;
use super::callback::GenerateTextCallbacks;
use super::callback::OnFinishEvent;
use super::callback::OnStartEvent;
use super::callback::OnStepStartEvent;
use super::callback::OnToolCallFinishEvent;
use super::callback::OnToolCallStartEvent;
use super::collect_tool_approvals::ToolApprovalCollector;
use super::collect_tool_approvals::ToolApprovalRequest;
use super::collect_tool_approvals::apply_approvals;
use super::content_utils;
use super::generate_text_result::GenerateTextResult;
use super::generate_text_result::ToolResult;
use super::output::Output;
use super::response_message::build_tool_result_message;
use super::step_result::StepResult;
use super::stop_condition::StopCondition;

/// A function that can override step configuration.
///
/// Called before each step to allow per-step model/tool/message overrides.
pub type PrepareStepFn =
    Arc<dyn Fn(&PrepareStepContext) -> Option<PrepareStepOverrides> + Send + Sync>;

/// Runtime context shared with prepare-step callbacks and tools.
pub type RuntimeContext = Arc<dyn Any + Send + Sync>;

/// Per-tool runtime contexts keyed by tool name.
pub type ToolContextMap = HashMap<String, RuntimeContext>;

/// Context provided to the `prepare_step` callback.
pub struct PrepareStepContext {
    /// The current step number.
    pub step: u32,
    /// Steps completed so far.
    pub steps: Vec<StepResult>,
    /// The current model ID.
    pub model_id: String,
    /// Messages that will be sent to the model for this step.
    pub messages: Vec<vercel_ai_provider::LanguageModelV4Message>,
    /// Shared runtime context for this step.
    pub runtime_context: Option<RuntimeContext>,
    /// Tool-specific runtime contexts for this step.
    pub tools_context: ToolContextMap,
}

impl std::fmt::Debug for PrepareStepContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrepareStepContext")
            .field("step", &self.step)
            .field("steps", &self.steps)
            .field("model_id", &self.model_id)
            .field("messages", &self.messages)
            .field("has_runtime_context", &self.runtime_context.is_some())
            .field("tools_context_len", &self.tools_context.len())
            .finish()
    }
}

/// Overrides returned from `prepare_step`.
#[derive(Default)]
pub struct PrepareStepOverrides {
    /// Override the tool choice for this step.
    pub tool_choice: Option<LanguageModelV4ToolChoice>,
    /// Override the active tools for this step.
    pub active_tools: Option<Vec<String>>,
    /// Override the model for this step.
    pub model: Option<crate::model::LanguageModel>,
    /// Override the system prompt for this step.
    pub system: Option<String>,
    /// Override provider options for this step.
    pub provider_options: Option<crate::types::ProviderOptions>,
    /// Override the entire messages array for this step.
    pub messages: Option<Vec<vercel_ai_provider::LanguageModelV4Message>>,
    /// Override the runtime context for this and subsequent work in the step.
    pub runtime_context: Option<RuntimeContext>,
    /// Override tool-specific runtime contexts for this step.
    pub tools_context: Option<ToolContextMap>,
}

/// Options for `generate_text`.
#[derive(Default)]
pub struct GenerateTextOptions {
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
    pub callbacks: GenerateTextCallbacks,
    /// Retry configuration for transient failures.
    pub retry_config: Option<RetryConfig>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Output configuration for structured output.
    pub output: Option<Output>,
    /// Stop conditions for multi-step generation.
    pub stop_when: Vec<StopCondition>,
    /// Filter which tools are available per step.
    pub active_tools: Option<Vec<String>>,
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

impl GenerateTextOptions {
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
    pub fn with_callbacks(mut self, callbacks: GenerateTextCallbacks) -> Self {
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

    /// Add a stop condition.
    pub fn with_stop_when(mut self, condition: StopCondition) -> Self {
        self.stop_when.push(condition);
        self
    }

    /// Set the active tools filter.
    pub fn with_active_tools(mut self, tools: Vec<String>) -> Self {
        self.active_tools = Some(tools);
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

/// Generate text from a prompt.
///
/// This function sends a prompt to a language model and returns the generated text.
/// It supports tool calling with automatic tool execution.
///
/// # Arguments
///
/// * `options` - The generation options including model, prompt, and settings.
///
/// # Returns
///
/// A `GenerateTextResult` containing the generated text, tool calls, and metadata.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{generate_text, GenerateTextOptions, LanguageModel, Prompt};
///
/// let result = generate_text(GenerateTextOptions {
///     model: "claude-3-sonnet".into(),
///     prompt: Prompt::user("Hello, world!"),
///     ..Default::default()
/// }).await?;
///
/// println!("Response: {}", result.text);
/// ```
#[tracing::instrument(skip_all)]
pub async fn generate_text(options: GenerateTextOptions) -> Result<GenerateTextResult, AIError> {
    let integrations_for_error = crate::telemetry::build_integrations(options.telemetry.as_ref());
    let call_id_for_error = vercel_ai_provider_utils::generate_id("call");

    match generate_text_inner(options, call_id_for_error.clone()).await {
        Ok(result) => Ok(result),
        Err(error) => {
            // Notify telemetry integrations of the error
            crate::telemetry::notify_error(&integrations_for_error, &error).await;
            Err(error)
        }
    }
}

async fn generate_text_inner(
    options: GenerateTextOptions,
    call_id: String,
) -> Result<GenerateTextResult, AIError> {
    // Resolve the model (this moves options.model, so do it first)
    let model = resolve_language_model(options.model)?;
    let model_id = model.model_id().to_string();
    let provider_id = model.provider().to_string();
    let model_info = CallbackModelInfo::new(&provider_id, &model_id);

    // Build telemetry integrations
    let integrations = crate::telemetry::build_integrations(options.telemetry.as_ref());

    // Extract telemetry function_id and metadata for setting on StepResults
    let telemetry_function_id = options
        .telemetry
        .as_ref()
        .and_then(|t| t.function_id.clone());
    let telemetry_metadata: Option<HashMap<String, serde_json::Value>> =
        options.telemetry.as_ref().and_then(|t| {
            t.metadata.as_ref().and_then(|m| {
                if let serde_json::Value::Object(map) = m {
                    Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                } else {
                    None
                }
            })
        });

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

    // Call on_start callback via telemetry dispatch
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

    // Track steps
    let max_steps = options.max_steps.unwrap_or(1);
    let mut steps: Vec<StepResult> = Vec::new();
    let mut total_usage = vercel_ai_provider::Usage::default();
    let mut runtime_context = options.runtime_context.clone();
    let mut tools_context = options.tools_context.clone();

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

    // Extract options needed for the loop
    let callbacks = &options.callbacks;
    let settings = &options.settings;
    let tool_choice = &options.tool_choice;
    let abort_signal = &options.abort_signal;
    let provider_options = &options.provider_options;
    let output = &options.output;

    // Multi-step loop
    for step in 0..max_steps {
        // Check for cancellation
        if let Some(signal) = abort_signal
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

        // Check timeout
        if let Some(ref timeout) = settings.timeout {
            check_timeout(timeout, step)?;
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
                .or_else(|| tool_choice.clone());
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
            step_tool_choice = tool_choice.clone();
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
            provider_options.as_ref(),
            step_provider_options.as_ref(),
        );

        // Filter active tools
        let effective_tools =
            build_call_options::filter_active_tools(&tool_definitions, &step_active_tools);

        // Call on_step_start callback (once per step, before the LLM call)
        crate::telemetry::notify_step_start(
            callbacks.on_step_start.as_deref(),
            &integrations,
            &OnStepStartEvent::new(&call_id, step, model_info.clone())
                .with_messages(step_messages.clone())
                .with_tools(tool_names.clone())
                .with_steps(steps.clone()),
        )
        .await;

        // Build call options using shared builder. abort_signal is threaded
        // separately to do_generate / do_stream — not part of CallOptions.
        let call_options = build_call_options::build_call_options(
            settings,
            &step_tool_choice,
            &effective_provider_options,
            output,
            step_messages,
            &effective_tools,
        );

        // Execute with retry, optionally with timeout
        let result = if let Some(ref timeout) = settings.timeout
            && let Some(step_ms) = timeout.step_ms
        {
            let duration = std::time::Duration::from_millis(step_ms);
            match tokio::time::timeout(
                duration,
                execute_with_retry(
                    effective_model,
                    &call_options,
                    retry_config.clone(),
                    abort_signal.clone(),
                ),
            )
            .await
            {
                Ok(r) => r?,
                Err(_) => {
                    return Err(AIError::Timeout(format!(
                        "Step {step} timed out after {step_ms}ms"
                    )));
                }
            }
        } else {
            execute_with_retry(
                effective_model,
                &call_options,
                retry_config.clone(),
                abort_signal.clone(),
            )
            .await?
        };

        // Log warnings from the provider (called unconditionally)
        crate::logger::log_warnings(&crate::logger::LogWarningsOptions::new(
            result.warnings.clone(),
            model.provider(),
            &model_id,
        ));

        // Update total usage
        total_usage.add(&result.usage);

        // Extract text and tool calls using shared utilities
        let text = content_utils::extract_text(&result.content);
        let mut tool_calls = content_utils::extract_tool_calls(&result.content);

        // Create step result with model info and telemetry fields
        let step_result = StepResult::new(
            step,
            text.clone(),
            result.usage.clone(),
            result.finish_reason.clone(),
        )
        .with_call_id(&call_id)
        .with_model(CallbackModelInfo::new(&provider_id, &model_id))
        .with_content(result.content.clone())
        .with_warnings(result.warnings.clone());
        let step_result = if let Some(ref fid) = telemetry_function_id {
            step_result.with_function_id(fid)
        } else {
            step_result
        };
        let step_result = if let Some(ref meta) = telemetry_metadata {
            step_result.with_metadata(meta.clone())
        } else {
            step_result
        };

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

            // Execute tool calls concurrently (TS uses Promise.all())
            // Fire on_tool_call_start for each tool before concurrent execution
            for tc in &tool_calls {
                crate::telemetry::notify_tool_call_start(
                    callbacks.on_tool_call_start.as_deref(),
                    &integrations,
                    &OnToolCallStartEvent::new(&call_id, step, model_info.clone(), tc.clone())
                        .with_messages(step_messages_for_tools.clone()),
                )
                .await;
            }

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
                let result = match result {
                    Ok(output) => ToolResult::new(&tc.tool_call_id, &tc.tool_name, output),
                    Err(e) => ToolResult::error(&tc.tool_call_id, &tc.tool_name, e.to_string()),
                };

                // Call on_tool_call_finish callback
                let finish_event = if result.is_error {
                    OnToolCallFinishEvent::error(
                        &call_id,
                        step,
                        model_info.clone(),
                        tc.clone(),
                        result.result.to_string(),
                        duration_ms,
                    )
                } else {
                    OnToolCallFinishEvent::success(
                        &call_id,
                        step,
                        model_info.clone(),
                        tc.clone(),
                        result.result.clone(),
                        duration_ms,
                    )
                };
                crate::telemetry::notify_tool_call_finish(
                    callbacks.on_tool_call_finish.as_deref(),
                    &integrations,
                    &finish_event,
                )
                .await;

                tool_results.push(result);
            }

            // Build step result with tool results
            let step_result = step_result
                .with_tool_calls(tool_calls.clone())
                .with_tool_results(tool_results.clone());

            // Call step finish callback via telemetry dispatch
            crate::telemetry::notify_step_finish(
                callbacks.on_step_finish.as_deref(),
                &integrations,
                &step_result,
            )
            .await;

            steps.push(step_result);

            // Add assistant message and tool results to conversation
            messages.push(vercel_ai_provider::LanguageModelV4Message::assistant(
                result.content.clone(),
            ));

            // Add tool results as tool messages using shared utility
            let tool_result_msg = build_tool_result_message(&tool_results);
            messages.push(tool_result_msg);

            // Continue to next step if we haven't reached max_steps
            continue;
        }

        // No tool calls or no tools available - finish

        // Call step finish callback via telemetry dispatch
        crate::telemetry::notify_step_finish(
            callbacks.on_step_finish.as_deref(),
            &integrations,
            &step_result,
        )
        .await;

        steps.push(step_result.clone());

        // Build final result
        let mut final_result =
            GenerateTextResult::from_generate_result(result, &model_id).with_steps(steps.clone());
        final_result.call_id = call_id.clone();
        final_result.total_usage = total_usage.clone();

        // Parse structured output if output spec is configured and finish reason is "stop"
        // Only parse output when finishReason === 'stop'
        if let Some(output_spec) = output
            && final_result.finish_reason.is_stop()
            && let Ok(Some(parsed)) = output_spec.parse_complete_output(&final_result.text)
        {
            final_result.output = Some(parsed);
        }

        // Call on_finish callback via telemetry dispatch
        let finish_event = OnFinishEvent::new(step_result, steps.clone(), total_usage);
        crate::telemetry::notify_finish(
            callbacks.on_finish.as_deref(),
            &integrations,
            &finish_event,
        )
        .await;

        return Ok(final_result);
    }

    // Reached max steps
    // Return the last step's result
    let last_step = steps.last().cloned().unwrap_or_else(|| {
        StepResult::new(
            max_steps - 1,
            String::new(),
            vercel_ai_provider::Usage::default(),
            vercel_ai_provider::FinishReason::end_turn(),
        )
    });

    let mut final_result = GenerateTextResult::new(
        last_step.text.clone(),
        total_usage.clone(),
        last_step.finish_reason.clone(),
    )
    .with_steps(steps.clone());
    final_result.call_id = call_id;
    final_result.total_usage = total_usage.clone();
    final_result.content = last_step.content.clone();
    final_result.reasoning = last_step.reasoning.clone();
    final_result.tool_calls = last_step.tool_calls.clone();
    final_result.tool_results = last_step.tool_results.clone();
    final_result.warnings = last_step.warnings.clone();
    final_result.provider_metadata = last_step.provider_metadata.clone();
    final_result.sources = last_step.sources.clone();
    final_result.files = last_step.files.clone();
    final_result.request = last_step.request.clone();
    final_result.response = last_step.response.clone();

    // Only parse structured output when finish reason is "stop"
    if let Some(output_spec) = output
        && final_result.finish_reason.is_stop()
        && let Ok(Some(parsed)) = output_spec.parse_complete_output(&final_result.text)
    {
        final_result.output = Some(parsed);
    }

    // Call on_finish callback via telemetry dispatch
    let finish_event = OnFinishEvent::new(last_step, steps, total_usage);
    crate::telemetry::notify_finish(callbacks.on_finish.as_deref(), &integrations, &finish_event)
        .await;

    Ok(final_result)
}

/// Execute a model call with retry logic.
///
/// `call_options` is borrowed across attempts — no per-attempt clone of
/// the prompt vector. `abort_signal` is `Arc`-backed so cloning per
/// attempt is a refcount bump.
async fn execute_with_retry(
    model: &Arc<dyn vercel_ai_provider::LanguageModelV4>,
    call_options: &LanguageModelV4CallOptions,
    retry_config: RetryConfig,
    abort_signal: Option<CancellationToken>,
) -> Result<vercel_ai_provider::LanguageModelV4GenerateResult, AIError> {
    let provider_name = model.provider().to_string();
    let model_id = model.model_id().to_string();

    with_retry(retry_config, abort_signal.clone(), || {
        let model = model.clone();
        let provider_name = provider_name.clone();
        let model_id = model_id.clone();
        let abort_signal = abort_signal.clone();
        async move {
            model
                .do_generate(call_options, abort_signal)
                .await
                .map_err(|e| crate::prompt::wrap_gateway_error(e, &provider_name, &model_id))
        }
    })
    .await
}

/// Check if a timeout has been exceeded.
fn check_timeout(timeout: &TimeoutConfiguration, _step: u32) -> Result<(), AIError> {
    // Validate the timeout configuration.
    // Actual per-step timeout enforcement is done via tokio::time::timeout in the loop.
    if let Some(total_ms) = timeout.total_ms
        && total_ms == 0
    {
        return Err(AIError::InvalidConfig(
            "total_ms timeout cannot be 0".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "generate_text.test.rs"]
mod tests;
