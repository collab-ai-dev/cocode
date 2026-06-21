use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Request;
use vercel_ai_provider::LanguageModelV4Response;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResponse;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::SourceType;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::StreamingToolCallDelta;
use vercel_ai_provider_utils::StreamingToolCallTracker;
use vercel_ai_provider_utils::ToolCallDeltaFunction;
use vercel_ai_provider_utils::TypeValidation;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::post_json_to_api_with_client_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_tapped;

use crate::openai_capabilities::SystemMessageMode;
use crate::openai_capabilities::get_capabilities;
use crate::openai_config::OpenAIConfig;
use crate::openai_error::OpenAIFailedResponseHandler;

use super::convert_chat_usage::convert_openai_chat_usage;
use super::convert_to_chat_messages::convert_to_openai_chat_messages;
use super::map_finish_reason::map_openai_chat_finish_reason;
use super::openai_chat_api::OpenAIChatChunk;
use super::openai_chat_api::OpenAIChatResponse;
use super::openai_chat_options::OpenAIChatProviderOptions;
use super::openai_chat_options::ReasoningEffort;
use super::openai_chat_options::extract_openai_options;
use super::prepare_tools::prepare_chat_tools;

/// OpenAI Chat Completions language model.
pub struct OpenAIChatLanguageModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAIChatLanguageModel {
    /// Create a new chat language model.
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Build request body and collect warnings.
    fn get_args(
        &self,
        options: &LanguageModelV4CallOptions,
    ) -> Result<(Value, Vec<Warning>), AISdkError> {
        let mut warnings = Vec::new();
        let (openai_options, raw_provider_options) =
            extract_openai_options(&options.provider_options);
        let caps = get_capabilities(&self.model_id);

        let force_reasoning = openai_options.force_reasoning.unwrap_or(false);
        let is_reasoning_model = force_reasoning || caps.is_reasoning_model;

        // topK is not supported by OpenAI
        if options.top_k.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "topK".into(),
                details: None,
            });
        }

        let system_message_mode =
            openai_options
                .system_message_mode
                .unwrap_or(if is_reasoning_model {
                    SystemMessageMode::Developer
                } else {
                    caps.system_message_mode
                });

        // Convert prompt to messages
        let (messages, msg_warnings) =
            convert_to_openai_chat_messages(&options.prompt, system_message_mode);
        warnings.extend(msg_warnings);

        // Prepare tools
        let prepared = prepare_chat_tools(&options.tools, &options.tool_choice);
        warnings.extend(prepared.warnings);

        // Build base body
        let mut body = json!({
            "model": self.model_id,
            "messages": messages,
        });

        // Add tools
        if let Some(tools) = prepared.tools {
            body["tools"] = Value::Array(tools);
        }
        if let Some(tc) = prepared.tool_choice {
            body["tool_choice"] = tc;
        }

        // Resolve reasoning effort: provider option takes precedence, then top-level reasoning.
        let reasoning_effort = openai_options.reasoning_effort.or_else(|| {
            if is_custom_reasoning(options.reasoning) {
                options.reasoning.and_then(|level| match level {
                    ReasoningLevel::Off => Some(ReasoningEffort::None),
                    ReasoningLevel::Minimal => Some(ReasoningEffort::Minimal),
                    ReasoningLevel::Low => Some(ReasoningEffort::Low),
                    ReasoningLevel::Medium => Some(ReasoningEffort::Medium),
                    ReasoningLevel::High => Some(ReasoningEffort::High),
                    ReasoningLevel::Xhigh => Some(ReasoningEffort::Xhigh),
                    ReasoningLevel::ProviderDefault => Option::None,
                })
            } else {
                Option::None
            }
        });
        let is_no_effort = reasoning_effort == Some(ReasoningEffort::None);
        let can_use_non_reasoning_params =
            is_no_effort && caps.supports_non_reasoning_params_with_no_effort;

        // Reasoning model parameter restrictions
        if is_reasoning_model {
            if let Some(effort) = reasoning_effort {
                body["reasoning_effort"] = Value::String(effort.as_str().into());
            }

            // Use max_completion_tokens instead of max_tokens
            if let Some(max) = openai_options
                .max_completion_tokens
                .or(options.max_output_tokens)
            {
                body["max_completion_tokens"] = json!(max);
            }

            // Only include these if effort is none and model supports it
            if can_use_non_reasoning_params {
                set_optional_f32(&mut body, "temperature", options.temperature);
                set_optional_f32(&mut body, "top_p", options.top_p);
                set_logprobs(&mut body, &openai_options);
            } else {
                // Emit warnings for silently dropped parameters
                if options.temperature.is_some() {
                    warnings.push(Warning::Unsupported {
                        feature: "temperature".into(),
                        details: Some("temperature is not supported for reasoning models".into()),
                    });
                }
                if options.top_p.is_some() {
                    warnings.push(Warning::Unsupported {
                        feature: "topP".into(),
                        details: Some("topP is not supported for reasoning models".into()),
                    });
                }
                if openai_options.logprobs.is_some() {
                    warnings.push(Warning::Other {
                        message: "logprobs is not supported for reasoning models".into(),
                    });
                    if matches!(
                        openai_options.logprobs,
                        Some(Value::Number(_)) | Some(Value::Bool(true))
                    ) {
                        warnings.push(Warning::Other {
                            message: "topLogprobs is not supported for reasoning models".into(),
                        });
                    }
                }
            }
            // Always omit: frequency_penalty, presence_penalty, logit_bias for reasoning
            if options.frequency_penalty.is_some() {
                warnings.push(Warning::Unsupported {
                    feature: "frequencyPenalty".into(),
                    details: Some("frequencyPenalty is not supported for reasoning models".into()),
                });
            }
            if options.presence_penalty.is_some() {
                warnings.push(Warning::Unsupported {
                    feature: "presencePenalty".into(),
                    details: Some("presencePenalty is not supported for reasoning models".into()),
                });
            }
            if openai_options.logit_bias.is_some() {
                warnings.push(Warning::Other {
                    message: "logitBias is not supported for reasoning models".into(),
                });
            }
        } else {
            // Non-reasoning model: include all parameters
            set_optional_f32(&mut body, "temperature", options.temperature);
            set_optional_f32(&mut body, "top_p", options.top_p);
            set_optional_f32(&mut body, "frequency_penalty", options.frequency_penalty);
            set_optional_f32(&mut body, "presence_penalty", options.presence_penalty);

            if let Some(max) = options.max_output_tokens {
                body["max_tokens"] = json!(max);
            }

            set_logprobs(&mut body, &openai_options);

            if let Some(ref bias) = openai_options.logit_bias {
                body["logit_bias"] = serde_json::to_value(bias).unwrap_or_default();
            }
        }

        // Search model handling — temperature is not supported
        if (self.model_id.starts_with("gpt-4o-search-preview")
            || self.model_id.starts_with("gpt-4o-mini-search-preview"))
            && body.get("temperature").is_some()
        {
            body.as_object_mut().map(|o| o.remove("temperature"));
            warnings.push(Warning::Unsupported {
                feature: "temperature".into(),
                details: Some(
                    "temperature is not supported for the search preview models and has been removed".into(),
                ),
            });
        }

        // Common fields
        if let Some(ref stop) = options.stop_sequences
            && !stop.is_empty()
        {
            body["stop"] = json!(stop);
        }
        if let Some(seed) = options.seed {
            body["seed"] = json!(seed);
        }

        // Provider-specific fields
        if let Some(ref user) = openai_options.user {
            body["user"] = Value::String(user.clone());
        }
        // Typed provider_options wins over the generic call-options
        // toggle so a user-explicit override beats the capability-driven
        // default flowing through `options.parallel_tool_calls`.
        if let Some(parallel) = openai_options
            .parallel_tool_calls
            .or(options.parallel_tool_calls)
        {
            body["parallel_tool_calls"] = Value::Bool(parallel);
        }
        if let Some(store) = openai_options.store {
            body["store"] = Value::Bool(store);
        }
        if let Some(ref metadata) = openai_options.metadata {
            body["metadata"] = serde_json::to_value(metadata).unwrap_or_default();
        }
        if let Some(ref prediction) = openai_options.prediction {
            body["prediction"] = prediction.clone();
        }
        if let Some(ref verbosity) = openai_options.text_verbosity {
            body["verbosity"] = Value::String(verbosity.as_str().into());
        }
        if let Some(ref cache_key) = openai_options.prompt_cache_key {
            body["prompt_cache_key"] = Value::String(cache_key.clone());
        }
        if let Some(ref retention) = openai_options.prompt_cache_retention {
            body["prompt_cache_retention"] = Value::String(retention.as_str().into());
        }
        if let Some(ref safety) = openai_options.safety_identifier {
            body["safety_identifier"] = Value::String(safety.clone());
        }

        // Service tier with validation
        if let Some(ref tier) = openai_options.service_tier {
            let mut set_tier = true;
            if *tier == super::openai_chat_options::ServiceTier::Flex
                && !caps.supports_flex_processing
            {
                warnings.push(Warning::Unsupported {
                    feature: "serviceTier".into(),
                    details: Some(
                        "flex processing is only available for o3, o4-mini, and gpt-5 models"
                            .into(),
                    ),
                });
                set_tier = false;
            }
            if *tier == super::openai_chat_options::ServiceTier::Priority
                && !caps.supports_priority_processing
            {
                warnings.push(Warning::Unsupported {
                    feature: "serviceTier".into(),
                    details: Some(
                        "priority processing is only available for supported models (gpt-4, gpt-5, gpt-5-mini, o3, o4-mini) and requires Enterprise access. gpt-5-nano is not supported".into(),
                    ),
                });
                set_tier = false;
            }
            if set_tier {
                body["service_tier"] = Value::String(tier.as_str().into());
            }
        }

        // Response format
        if let Some(ref format) = options.response_format {
            match format {
                ResponseFormat::Text => {
                    body["response_format"] = json!({"type": "text"});
                }
                ResponseFormat::Json {
                    schema,
                    name,
                    description,
                } => {
                    if let Some(schema) = schema {
                        let strict = openai_options.strict_json_schema.unwrap_or(true);
                        let schema_name = name.as_deref().unwrap_or("response");
                        let mut json_schema = json!({
                            "type": "json_schema",
                            "json_schema": {
                                "schema": schema,
                                "strict": strict,
                                "name": schema_name,
                            }
                        });
                        if let Some(desc) = description {
                            json_schema["json_schema"]["description"] = Value::String(desc.clone());
                        }
                        body["response_format"] = json_schema;
                    } else {
                        body["response_format"] = json!({"type": "json_object"});
                    }
                }
            }
        }

        // Deep-merge extra_body onto the wire body. Producers
        // (`coco_inference::thinking_convert`, user extras) own the
        // wire-correct nesting; deep merge places nested overlays at
        // the right slot without clobbering sibling typed writes.
        if !raw_provider_options.is_empty() {
            let overlay = Value::Object(raw_provider_options.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        Ok((body, warnings))
    }
}

#[async_trait]
impl LanguageModelV4 for OpenAIChatLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        let mut map = HashMap::new();
        // OpenAI chat supports image URLs
        if let Ok(re) = Regex::new(r"^https?://.*$") {
            map.insert("image/*".into(), vec![re]);
        }
        map
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let (body, warnings) = self.get_args(options)?;
        let url = self.config.url("/chat/completions");
        let headers = self.config.get_headers();

        let response: OpenAIChatResponse = post_json_to_api_with_client_tapped(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            OpenAIFailedResponseHandler,
            abort_signal,
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        // Extract content from first choice
        let choice = response
            .choices
            .first()
            .ok_or_else(|| AISdkError::new("No choices in response"))?;

        let mut content: Vec<AssistantContentPart> = Vec::new();

        // Text content
        if let Some(ref text) = choice.message.content
            && !text.is_empty()
        {
            content.push(AssistantContentPart::Text(TextPart {
                text: text.clone(),
                provider_metadata: None,
            }));
        }

        // Tool calls — route raw `arguments` through `llm_json`-backed
        // repair. On parse failure, fall back to `Value::Object({})`
        // (not `invalid = true`) so schema validation in
        // `app/query` reports the specific missing fields instead of
        // just "JSON broken".
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let input = vercel_ai_provider_utils::parse_tool_arguments_or_empty(
                    &tc.function.arguments,
                    &tc.function.name,
                );
                let tool_call_id = tc
                    .id
                    .clone()
                    .unwrap_or_else(|| vercel_ai_provider_utils::generate_id("tc"));
                content.push(AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id,
                    tool_name: tc.function.name.clone(),
                    input,
                    provider_executed: None,
                    invalid: false,
                    invalid_reason: None,
                    provider_metadata: None,
                }));
            }
        }

        // Sources from annotations
        if let Some(ref annotations) = choice.message.annotations {
            for ann in annotations {
                if ann.annotation_type.as_deref() == Some("url_citation")
                    && let Some(ref citation) = ann.url_citation
                {
                    content.push(AssistantContentPart::Source(
                        vercel_ai_provider::content::SourcePart {
                            source_type: SourceType::Url,
                            id: vercel_ai_provider_utils::generate_id("src"),
                            url: citation.url.clone(),
                            title: citation.title.clone(),
                            media_type: None,
                            filename: None,
                            provider_metadata: None,
                        },
                    ));
                }
            }
        }

        let finish_reason = map_openai_chat_finish_reason(choice.finish_reason.as_deref());
        let usage = convert_openai_chat_usage(response.usage.as_ref());

        // Provider metadata (nested under "openai" key)
        let mut openai_obj = serde_json::Map::new();
        if let Some(ref usage_data) = response.usage
            && let Some(ref details) = usage_data.completion_tokens_details
        {
            if let Some(accepted) = details.accepted_prediction_tokens {
                openai_obj.insert("acceptedPredictionTokens".into(), json!(accepted));
            }
            if let Some(rejected) = details.rejected_prediction_tokens {
                openai_obj.insert("rejectedPredictionTokens".into(), json!(rejected));
            }
        }
        if let Some(ref logprobs) = choice.logprobs
            && let Ok(v) = serde_json::to_value(logprobs)
        {
            openai_obj.insert("logprobs".into(), v);
        }
        if let Some(ref tier) = response.service_tier {
            openai_obj.insert("serviceTier".into(), Value::String(tier.clone()));
        }
        let provider_metadata = if openai_obj.is_empty() {
            None
        } else {
            let mut meta = ProviderMetadata::default();
            meta.0.insert("openai".into(), Value::Object(openai_obj));
            Some(meta)
        };

        // Response metadata
        let timestamp = response
            .created
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0));

        Ok(LanguageModelV4GenerateResult {
            content,
            usage,
            finish_reason,
            warnings,
            provider_metadata,
            request: Some(LanguageModelV4Request { body: Some(body) }),
            response: Some(LanguageModelV4Response {
                id: response.id.clone(),
                timestamp,
                model_id: response.model,
                headers: None,
                body: None,
            }),
        })
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        let (mut body, warnings) = self.get_args(options)?;

        // Enable streaming
        body["stream"] = Value::Bool(true);
        body["stream_options"] = json!({"include_usage": true});

        let include_raw = options.include_raw_chunks.unwrap_or(false);

        let url = self.config.url("/chat/completions");
        let headers = self.config.get_headers();

        let byte_stream = post_stream_to_api_with_client_tapped(
            &url,
            Some(headers),
            &body,
            abort_signal,
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        let request_body = body.clone();

        let stream = create_chat_stream(byte_stream, warnings, include_raw);

        Ok(LanguageModelV4StreamResult {
            stream,
            request: Some(LanguageModelV4Request {
                body: Some(request_body),
            }),
            response: Some(LanguageModelV4StreamResponse::new()),
        })
    }
}

/// Create a stream of `LanguageModelV4StreamPart` from a raw SSE byte stream.
fn create_chat_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
    include_raw: bool,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    let stream = futures::stream::unfold(
        ChatStreamState::new(byte_stream, warnings, include_raw),
        |mut state| async move {
            loop {
                // If we have queued events, yield them first
                if let Some(event) = state.pending.pop_front() {
                    return Some((Ok(event), state));
                }

                // If done and all pending events drained, yield nothing
                if state.done && state.pending.is_empty() {
                    return None;
                }

                // Read more bytes and parse SSE lines
                match state.next_events().await {
                    Ok(true) => {
                        // Stream still open, loop back to drain pending or read more
                    }
                    Ok(false) => {
                        // Byte stream ended
                        state.done = true;
                        // Emit finish if we haven't already
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            let pm = if state.provider_metadata.is_empty() {
                                None
                            } else {
                                let inner = Value::Object(
                                    std::mem::take(&mut state.provider_metadata)
                                        .into_iter()
                                        .collect(),
                                );
                                let mut meta = ProviderMetadata::default();
                                meta.0.insert("openai".into(), inner);
                                Some(meta)
                            };
                            let finish = LanguageModelV4StreamPart::Finish {
                                usage: convert_openai_chat_usage(state.usage.as_ref()),
                                finish_reason: map_openai_chat_finish_reason(
                                    state.finish_reason.as_deref(),
                                ),
                                provider_metadata: pm,
                            };
                            state.pending.push_back(finish);
                        }
                        // Fall through to loop — drain pending
                    }
                    Err(e) => {
                        state.done = true;
                        return Some((Err(e), state));
                    }
                }
            }
        },
    );

    Box::pin(stream)
}

struct ChatStreamState {
    byte_stream: vercel_ai_provider_utils::ByteStream,
    buffer: String,
    pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
    /// Shared tool-call accumulator.
    /// OpenAI Chat is strict — id + function.name on first delta —
    /// which matches the tracker's default semantics, so we use it
    /// directly without the outer pending buffer that
    /// openai-compatible needs.
    tool_call_tracker: StreamingToolCallTracker,
    text_started: bool,
    text_id: String,
    usage: Option<super::convert_chat_usage::OpenAIChatUsage>,
    finish_reason: Option<String>,
    finish_emitted: bool,
    done: bool,
    metadata_emitted: bool,
    include_raw: bool,
    provider_metadata: HashMap<String, Value>,
}

impl ChatStreamState {
    fn new(
        byte_stream: vercel_ai_provider_utils::ByteStream,
        warnings: Vec<Warning>,
        include_raw: bool,
    ) -> Self {
        let mut pending = std::collections::VecDeque::new();
        pending.push_back(LanguageModelV4StreamPart::StreamStart { warnings });

        let tool_call_tracker = StreamingToolCallTracker::with_options(
            vercel_ai_provider_utils::StreamingToolCallTrackerOptions {
                type_validation: TypeValidation::IfPresent,
                ..Default::default()
            },
        );

        Self {
            byte_stream,
            buffer: String::new(),
            pending,
            tool_call_tracker,
            text_started: false,
            text_id: vercel_ai_provider_utils::generate_id("txt"),
            usage: None,
            finish_reason: None,
            finish_emitted: false,
            done: false,
            metadata_emitted: false,
            include_raw,
            provider_metadata: HashMap::new(),
        }
    }

    /// Drain queued tool-call stream parts from the tracker into the
    /// state's `pending` deque. Called after every `process_delta` /
    /// `flush` so ordering stays consistent with surrounding text.
    fn drain_tool_call_parts(&mut self) {
        for part in self.tool_call_tracker.take_parts() {
            self.pending.push_back(part);
        }
    }

    /// Read from byte_stream, parse SSE lines, produce events.
    /// Returns Ok(true) if the stream is still open, Ok(false) if the stream ended.
    async fn next_events(&mut self) -> Result<bool, AISdkError> {
        use futures::StreamExt;

        match self.byte_stream.next().await {
            Some(Ok(bytes)) => {
                let text = String::from_utf8_lossy(&bytes);
                self.buffer.push_str(&text);
                self.process_buffer();
                Ok(true)
            }
            Some(Err(e)) => Err(AISdkError::new(format!("Stream read error: {e}"))),
            None => Ok(false),
        }
    }

    /// Process accumulated buffer, extracting complete SSE data lines.
    fn process_buffer(&mut self) {
        while let Some(line_end) = self.buffer.find('\n') {
            let line_len = line_end + 1;
            let line = self.buffer[..line_end].trim_end_matches('\r');
            if let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
                && !data.is_empty()
                && data != "[DONE]"
            {
                let data = data.to_string();
                self.buffer.drain(..line_len);
                self.process_data_line(&data);
                continue;
            }
            self.buffer.drain(..line_len);
        }
    }

    /// Process a single SSE data JSON line.
    fn process_data_line(&mut self, data: &str) {
        let chunk: OpenAIChatChunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(_) => return,
        };

        // Emit raw chunk if requested
        if self.include_raw
            && let Ok(raw) = serde_json::from_str::<Value>(data)
        {
            self.pending
                .push_back(LanguageModelV4StreamPart::Raw { raw_value: raw });
        }

        // Emit response metadata once
        if !self.metadata_emitted {
            self.metadata_emitted = true;
            let timestamp = chunk
                .created
                .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0))
                .map(|dt| dt.to_rfc3339());

            let mut meta = ResponseMetadata::new();
            if let Some(ref id) = chunk.id {
                meta = meta.with_id(id.clone());
            }
            if let Some(ref model) = chunk.model {
                meta = meta.with_model(model.clone());
            }
            if let Some(ts) = timestamp {
                meta = meta.with_timestamp(ts);
            }
            if let Some(ref tier) = chunk.service_tier {
                let mut openai_obj = serde_json::Map::new();
                openai_obj.insert("serviceTier".into(), Value::String(tier.clone()));
                let mut pm = ProviderMetadata::default();
                pm.0.insert("openai".into(), Value::Object(openai_obj));
                meta = meta.with_provider_metadata(pm);
            }
            self.pending
                .push_back(LanguageModelV4StreamPart::ResponseMetadata(meta));
        }

        // Usage (comes in the final chunk)
        if let Some(ref u) = chunk.usage {
            if let Some(ref details) = u.completion_tokens_details {
                if let Some(accepted) = details.accepted_prediction_tokens {
                    self.provider_metadata
                        .insert("acceptedPredictionTokens".into(), json!(accepted));
                }
                if let Some(rejected) = details.rejected_prediction_tokens {
                    self.provider_metadata
                        .insert("rejectedPredictionTokens".into(), json!(rejected));
                }
            }
            self.usage = Some(u.clone());
        }

        // Process choices
        if let Some(ref choices) = chunk.choices {
            for choice in choices {
                // Track finish reason
                if let Some(ref fr) = choice.finish_reason {
                    self.finish_reason = Some(fr.clone());
                }

                // Extract logprobs
                if let Some(ref logprobs) = choice.logprobs
                    && let Ok(v) = serde_json::to_value(logprobs)
                {
                    self.provider_metadata.insert("logprobs".into(), v);
                }

                // Skip if delta is null/absent
                let Some(ref delta) = choice.delta else {
                    continue;
                };

                // Text delta
                if let Some(ref content) = delta.content
                    && !content.is_empty()
                {
                    if !self.text_started {
                        self.text_started = true;
                        self.pending
                            .push_back(LanguageModelV4StreamPart::TextStart {
                                id: self.text_id.clone(),
                                provider_metadata: None,
                            });
                    }
                    self.pending
                        .push_back(LanguageModelV4StreamPart::TextDelta {
                            id: self.text_id.clone(),
                            delta: content.clone(),
                            provider_metadata: None,
                        });
                }

                // Tool call deltas — close any open text first so
                // tool-input-start lands after text-end (matches TS).
                if let Some(ref tool_calls) = delta.tool_calls
                    && !tool_calls.is_empty()
                {
                    if self.text_started {
                        self.text_started = false;
                        self.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                            id: self.text_id.clone(),
                            provider_metadata: None,
                        });
                    }

                    for tc_delta in tool_calls {
                        let function = tc_delta.function.as_ref().map(|f| ToolCallDeltaFunction {
                            name: f.name.clone(),
                            arguments: f.arguments.clone(),
                        });
                        let delta_for_tracker = StreamingToolCallDelta {
                            index: Some(tc_delta.index as usize),
                            id: tc_delta.id.clone(),
                            r#type: tc_delta.tool_type.clone(),
                            function,
                            extra: None,
                        };
                        if let Err(e) = self.tool_call_tracker.process_delta(delta_for_tracker) {
                            self.pending.push_back(LanguageModelV4StreamPart::Error {
                                error: vercel_ai_provider::StreamError::new(e.message),
                            });
                            self.done = true;
                            return;
                        }
                    }
                    self.drain_tool_call_parts();
                }

                // Annotations (sources)
                if let Some(ref annotations) = delta.annotations {
                    for ann in annotations {
                        if ann.annotation_type.as_deref() == Some("url_citation")
                            && let Some(ref citation) = ann.url_citation
                        {
                            self.pending.push_back(LanguageModelV4StreamPart::Source(
                                vercel_ai_provider::content::SourcePart {
                                    source_type: SourceType::Url,
                                    id: vercel_ai_provider_utils::generate_id("src"),
                                    url: citation.url.clone(),
                                    title: citation.title.clone(),
                                    media_type: None,
                                    filename: None,
                                    provider_metadata: None,
                                },
                            ));
                        }
                    }
                }
            }
        }

        // When stream is ending, close open text and finalize tool calls
        if self.finish_reason.is_some() {
            if self.text_started {
                self.text_started = false;
                self.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                    id: self.text_id.clone(),
                    provider_metadata: None,
                });
            }

            // Finalize any tool calls the tracker accumulated but
            // didn't auto-finish (partial JSON, etc.).
            self.tool_call_tracker.flush();
            self.drain_tool_call_parts();
        }
    }
}

fn set_optional_f32(body: &mut Value, key: &str, value: Option<f32>) {
    if let Some(v) = value {
        body[key] = json!(v);
    }
}

fn set_logprobs(body: &mut Value, options: &OpenAIChatProviderOptions) {
    if let Some(ref logprobs) = options.logprobs {
        match logprobs {
            Value::Bool(true) => {
                body["logprobs"] = Value::Bool(true);
                body["top_logprobs"] = json!(0);
            }
            Value::Number(n) => {
                body["logprobs"] = Value::Bool(true);
                body["top_logprobs"] = Value::Number(n.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "openai_chat_language_model.test.rs"]
mod tests;
