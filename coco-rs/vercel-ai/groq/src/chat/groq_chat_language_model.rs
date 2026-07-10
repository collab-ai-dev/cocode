use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;

use async_trait::async_trait;
use futures::Stream;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Request;
use vercel_ai_provider::LanguageModelV4Response;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResponse;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::StreamError;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::UnifiedFinishReason;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::StreamingToolCallDelta;
use vercel_ai_provider_utils::StreamingToolCallTracker;
use vercel_ai_provider_utils::ToolCallDeltaFunction;
use vercel_ai_provider_utils::generate_id;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::map_reasoning_to_provider_effort;
use vercel_ai_provider_utils::post_json_to_api_with_client_and_headers_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_and_headers_tapped;

use crate::convert_groq_usage::GroqUsage;
use crate::convert_groq_usage::convert_groq_usage;
use crate::groq_config::GroqConfig;
use crate::groq_error::GroqFailedResponseHandler;
use crate::map_groq_finish_reason::map_groq_finish_reason;

use super::convert_to_groq_chat_messages::convert_to_groq_chat_messages;
use super::groq_api_types::GroqChatChunk;
use super::groq_api_types::GroqChatChunkToolCall;
use super::groq_api_types::GroqChatResponse;
use super::groq_chat_options::extract_groq_chat_options;
use super::groq_prepare_tools::prepare_groq_tools;

/// Groq accepts any `https?://` image URL. Compiled once; `Regex` clones share
/// the underlying automaton, so `supported_urls()` stays cheap.
static IMAGE_URL_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^https?://.*$").ok());

/// Maps a provider-agnostic `ReasoningLevel` to Groq's `reasoning_effort`.
/// Groq has no `minimal`/`xhigh` tiers, so they fold into `low`/`high`.
static REASONING_EFFORT_MAP: LazyLock<HashMap<ReasoningLevel, &'static str>> =
    LazyLock::new(|| {
        HashMap::from([
            (ReasoningLevel::Minimal, "low"),
            (ReasoningLevel::Low, "low"),
            (ReasoningLevel::Medium, "medium"),
            (ReasoningLevel::High, "high"),
            (ReasoningLevel::Xhigh, "high"),
        ])
    });

/// Groq Chat Completions language model.
pub struct GroqChatLanguageModel {
    model_id: String,
    config: Arc<GroqConfig>,
}

impl GroqChatLanguageModel {
    /// Create a new Groq chat language model.
    pub fn new(model_id: impl Into<String>, config: Arc<GroqConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Build the request body and collect warnings. Public so cross-crate
    /// tests can inspect the wire shape without dispatching HTTP.
    pub fn get_args(
        &self,
        options: &LanguageModelV4CallOptions,
    ) -> Result<(Value, Vec<Warning>), AISdkError> {
        let mut warnings = Vec::new();
        let groq_options = extract_groq_chat_options(&options.provider_options);

        let structured_outputs = groq_options.structured_outputs.unwrap_or(true);
        let strict_json_schema = groq_options.strict_json_schema.unwrap_or(true);

        if options.top_k.is_some() {
            warnings.push(Warning::unsupported("topK"));
        }

        // Convert prompt to Groq messages.
        let (messages, msg_warnings) = convert_to_groq_chat_messages(&options.prompt)?;
        warnings.extend(msg_warnings);

        // Prepare tools (browser search aware).
        let prepared = prepare_groq_tools(&options.tools, &options.tool_choice, &self.model_id);
        warnings.extend(prepared.warnings);

        let mut body = json!({
            "model": self.model_id,
            "messages": messages,
        });

        if let Some(tools) = prepared.tools {
            body["tools"] = Value::Array(tools);
        }
        if let Some(tc) = prepared.tool_choice {
            body["tool_choice"] = tc;
        }

        // Model-specific settings.
        if let Some(ref user) = groq_options.user {
            body["user"] = Value::String(user.clone());
        }
        if let Some(parallel) = groq_options.parallel_tool_calls {
            body["parallel_tool_calls"] = Value::Bool(parallel);
        }

        // Standardized settings.
        if let Some(max) = options.max_output_tokens {
            body["max_tokens"] = json!(max);
        }
        set_optional_f32(&mut body, "temperature", options.temperature);
        set_optional_f32(&mut body, "top_p", options.top_p);
        set_optional_f32(&mut body, "frequency_penalty", options.frequency_penalty);
        set_optional_f32(&mut body, "presence_penalty", options.presence_penalty);
        if let Some(ref stop) = options.stop_sequences
            && !stop.is_empty()
        {
            body["stop"] = json!(stop);
        }
        if let Some(seed) = options.seed {
            body["seed"] = json!(seed);
        }

        // Response format.
        if let Some(ResponseFormat::Json {
            schema,
            name,
            description,
        }) = &options.response_format
        {
            match schema {
                Some(schema) if structured_outputs => {
                    let mut json_schema = json!({
                        "schema": schema,
                        "strict": strict_json_schema,
                        "name": name.as_deref().unwrap_or("response"),
                    });
                    if let Some(desc) = description {
                        json_schema["description"] = Value::String(desc.clone());
                    }
                    body["response_format"] = json!({
                        "type": "json_schema",
                        "json_schema": json_schema,
                    });
                }
                _ => {
                    if schema.is_some() && !structured_outputs {
                        warnings.push(Warning::unsupported_with_details(
                            "responseFormat",
                            "JSON response format schema is only supported with structuredOutputs",
                        ));
                    }
                    body["response_format"] = json!({ "type": "json_object" });
                }
            }
        }

        // Provider options.
        if let Some(ref reasoning_format) = groq_options.reasoning_format {
            body["reasoning_format"] = Value::String(reasoning_format.clone());
        }
        if let Some(effort) = self.resolve_reasoning_effort(&groq_options, options, &mut warnings) {
            body["reasoning_effort"] = Value::String(effort);
        }
        if let Some(ref service_tier) = groq_options.service_tier {
            body["service_tier"] = Value::String(service_tier.clone());
        }

        Ok((body, warnings))
    }

    /// Resolve `reasoning_effort`: the explicit provider option wins;
    /// otherwise map a top-level `ReasoningLevel` (skipping `off`).
    fn resolve_reasoning_effort(
        &self,
        groq_options: &super::groq_chat_options::GroqChatProviderOptions,
        options: &LanguageModelV4CallOptions,
        warnings: &mut Vec<Warning>,
    ) -> Option<String> {
        if let Some(ref effort) = groq_options.reasoning_effort {
            return Some(effort.clone());
        }
        let level = options.reasoning?;
        if !is_custom_reasoning(Some(level)) || level == ReasoningLevel::Off {
            return None;
        }
        map_reasoning_to_provider_effort(level, &REASONING_EFFORT_MAP, warnings)
    }
}

#[async_trait]
impl LanguageModelV4 for GroqChatLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        // Groq accepts image URLs: `{ 'image/*': [/^https?:\/\/.*$/] }`.
        let mut map = HashMap::new();
        if let Some(re) = IMAGE_URL_RE.as_ref() {
            map.insert("image/*".to_string(), vec![re.clone()]);
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

        let api_response = post_json_to_api_with_client_and_headers_tapped::<GroqChatResponse>(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            GroqFailedResponseHandler::new(),
            abort_signal,
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        let response = api_response.value;
        let response_headers = api_response.headers;

        let choice = response
            .choices
            .first()
            .ok_or_else(|| AISdkError::new("No choices in Groq response"))?;

        let mut content: Vec<AssistantContentPart> = Vec::new();

        // Text (before reasoning, matching TS order).
        if let Some(ref text) = choice.message.content
            && !text.is_empty()
        {
            content.push(AssistantContentPart::Text(TextPart::new(text.clone())));
        }

        // Reasoning.
        if let Some(ref reasoning) = choice.message.reasoning
            && !reasoning.is_empty()
        {
            content.push(AssistantContentPart::Reasoning(ReasoningPart::new(
                reasoning.clone(),
            )));
        }

        // Tool calls.
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let input = vercel_ai_provider_utils::parse_tool_arguments_or_empty(
                    &tc.function.arguments,
                    &tc.function.name,
                );
                content.push(AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id: tc.id.clone().unwrap_or_else(|| generate_id("call")),
                    tool_name: tc.function.name.clone(),
                    input,
                    provider_executed: None,
                    invalid: false,
                    invalid_reason: None,
                    provider_metadata: None,
                }));
            }
        }

        let finish_reason = map_groq_finish_reason(choice.finish_reason.as_deref());
        let usage = convert_groq_usage(response.usage.as_ref());

        let response_body = serde_json::to_value(&response).ok();
        let timestamp = response
            .created
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0));

        Ok(LanguageModelV4GenerateResult {
            content,
            usage,
            finish_reason,
            warnings,
            provider_metadata: None,
            request: Some(LanguageModelV4Request { body: Some(body) }),
            response: Some(LanguageModelV4Response {
                id: response.id,
                timestamp,
                model_id: response.model,
                headers: Some(response_headers),
                body: response_body,
            }),
        })
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        let (mut body, warnings) = self.get_args(options)?;
        body["stream"] = Value::Bool(true);

        let include_raw = options.include_raw_chunks.unwrap_or(false);
        let url = self.config.url("/chat/completions");
        let headers = self.config.get_headers();

        let (byte_stream, response_headers) = post_stream_to_api_with_client_and_headers_tapped(
            &url,
            Some(headers),
            &body,
            abort_signal,
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        let request_body = body.clone();
        let stream = create_groq_chat_stream(byte_stream, warnings, include_raw);

        Ok(LanguageModelV4StreamResult {
            stream,
            request: Some(LanguageModelV4Request {
                body: Some(request_body),
            }),
            response: Some(LanguageModelV4StreamResponse {
                headers: Some(response_headers),
            }),
        })
    }
}

/// Create a `LanguageModelV4StreamPart` stream from a raw Groq SSE byte stream.
fn create_groq_chat_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
    include_raw: bool,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    let stream = futures::stream::unfold(
        GroqStreamState::new(byte_stream, warnings, include_raw),
        |mut state| async move {
            loop {
                if let Some(event) = state.pending.pop_front() {
                    return Some((Ok(event), state));
                }
                if state.done && state.pending.is_empty() {
                    return None;
                }
                match state.next_events().await {
                    Ok(true) => {}
                    Ok(false) => {
                        state.done = true;
                        state.close_reasoning();
                        state.close_text();
                        state.tool_call_tracker.flush();
                        state.drain_tool_call_parts();
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            let finish_reason = if state.stream_errored {
                                FinishReason {
                                    unified: UnifiedFinishReason::Error,
                                    raw: None,
                                }
                            } else {
                                map_groq_finish_reason(state.finish_reason.as_deref())
                            };
                            state.pending.push_back(LanguageModelV4StreamPart::Finish {
                                usage: convert_groq_usage(state.usage.as_ref()),
                                finish_reason,
                                provider_metadata: None,
                            });
                        }
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

struct GroqStreamState {
    byte_stream: vercel_ai_provider_utils::ByteStream,
    decoder: vercel_ai_provider_utils::SseDecoder,
    pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
    tool_call_tracker: StreamingToolCallTracker,
    text_started: bool,
    text_id: String,
    reasoning_started: bool,
    reasoning_id: String,
    usage: Option<GroqUsage>,
    finish_reason: Option<String>,
    /// Set when a chunk fails to parse or the API sends an error chunk. Drives
    /// a `UnifiedFinishReason::Error` finish instead of routing a fake wire
    /// string through the finish-reason mapper.
    stream_errored: bool,
    finish_emitted: bool,
    done: bool,
    metadata_emitted: bool,
    include_raw: bool,
}

impl GroqStreamState {
    fn new(
        byte_stream: vercel_ai_provider_utils::ByteStream,
        warnings: Vec<Warning>,
        include_raw: bool,
    ) -> Self {
        let mut pending = std::collections::VecDeque::new();
        pending.push_back(LanguageModelV4StreamPart::StreamStart { warnings });
        Self {
            byte_stream,
            decoder: vercel_ai_provider_utils::SseDecoder::new(),
            pending,
            tool_call_tracker: StreamingToolCallTracker::new(),
            text_started: false,
            text_id: "txt-0".to_string(),
            reasoning_started: false,
            reasoning_id: "reasoning-0".to_string(),
            usage: None,
            finish_reason: None,
            stream_errored: false,
            finish_emitted: false,
            done: false,
            metadata_emitted: false,
            include_raw,
        }
    }

    async fn next_events(&mut self) -> Result<bool, AISdkError> {
        use futures::StreamExt;
        match self.byte_stream.next().await {
            Some(Ok(bytes)) => {
                self.decoder.push(&bytes);
                while let Some(data) = self.decoder.next_data_line() {
                    self.process_data_line(&data);
                }
                Ok(true)
            }
            Some(Err(e)) => Err(AISdkError::new(format!("Stream read error: {e}"))),
            None => Ok(false),
        }
    }

    fn drain_tool_call_parts(&mut self) {
        for part in self.tool_call_tracker.take_parts() {
            self.pending.push_back(part);
        }
    }

    fn close_reasoning(&mut self) {
        if self.reasoning_started {
            self.reasoning_started = false;
            self.pending
                .push_back(LanguageModelV4StreamPart::ReasoningEnd {
                    id: self.reasoning_id.clone(),
                    provider_metadata: None,
                });
        }
    }

    fn close_text(&mut self) {
        if self.text_started {
            self.text_started = false;
            self.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                id: self.text_id.clone(),
                provider_metadata: None,
            });
        }
    }

    fn forward_tool_call(&mut self, tc: &GroqChatChunkToolCall) {
        let function = tc.function.as_ref().map(|f| ToolCallDeltaFunction {
            name: f.name.clone(),
            arguments: f.arguments.clone(),
        });
        let delta = StreamingToolCallDelta {
            index: tc.index.map(|i| i as usize),
            id: tc.id.clone(),
            r#type: tc.tool_type.clone(),
            function,
            extra: None,
        };
        if let Err(e) = self.tool_call_tracker.process_delta(delta) {
            self.pending.push_back(LanguageModelV4StreamPart::Error {
                error: StreamError::new(format!("Invalid response data: {}", e.message)),
            });
        }
        self.drain_tool_call_parts();
    }

    fn process_data_line(&mut self, data: &str) {
        let raw: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                self.stream_errored = true;
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: StreamError::new(format!("Failed to parse Groq chunk: {e}")),
                });
                return;
            }
        };

        if self.include_raw {
            self.pending.push_back(LanguageModelV4StreamPart::Raw {
                raw_value: raw.clone(),
            });
        }

        if let Some(error) = raw.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            self.finish_reason = Some("error".to_string());
            self.pending.push_back(LanguageModelV4StreamPart::Error {
                error: StreamError::new(message),
            });
            return;
        }

        let chunk: GroqChatChunk = match serde_json::from_value(raw) {
            Ok(c) => c,
            Err(e) => {
                self.stream_errored = true;
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: StreamError::new(format!("Invalid Groq chunk structure: {e}")),
                });
                return;
            }
        };

        if !self.metadata_emitted {
            self.metadata_emitted = true;
            let mut meta = ResponseMetadata::new();
            if let Some(ref id) = chunk.id {
                meta = meta.with_id(id.clone());
            }
            if let Some(ref model) = chunk.model {
                meta = meta.with_model(model.clone());
            }
            if let Some(ts) = chunk
                .created
                .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0))
                .map(|dt| dt.to_rfc3339())
            {
                meta = meta.with_timestamp(ts);
            }
            self.pending
                .push_back(LanguageModelV4StreamPart::ResponseMetadata(meta));
        }

        // Groq streams usage under `x_groq.usage`.
        if let Some(ref x_groq) = chunk.x_groq
            && let Some(ref u) = x_groq.usage
        {
            self.usage = Some(u.clone());
        }

        let Some(ref choices) = chunk.choices else {
            return;
        };
        for choice in choices {
            if let Some(ref fr) = choice.finish_reason {
                self.finish_reason = Some(fr.clone());
            }
            let Some(ref delta) = choice.delta else {
                continue;
            };

            if let Some(ref reasoning) = delta.reasoning
                && !reasoning.is_empty()
            {
                if !self.reasoning_started {
                    self.reasoning_started = true;
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningStart {
                            id: self.reasoning_id.clone(),
                            provider_metadata: None,
                        });
                }
                self.pending
                    .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                        id: self.reasoning_id.clone(),
                        delta: reasoning.clone(),
                        provider_metadata: None,
                    });
            }

            if let Some(ref content) = delta.content
                && !content.is_empty()
            {
                self.close_reasoning();
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

            if let Some(ref tool_calls) = delta.tool_calls {
                self.close_reasoning();
                for tc in tool_calls {
                    self.forward_tool_call(tc);
                }
            }
        }
    }
}

fn set_optional_f32(body: &mut Value, key: &str, value: Option<f32>) {
    if let Some(v) = value {
        body[key] = json!(v);
    }
}

#[cfg(test)]
#[path = "groq_chat_language_model.test.rs"]
mod tests;
