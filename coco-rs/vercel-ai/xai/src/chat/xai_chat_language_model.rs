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
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Request;
use vercel_ai_provider::LanguageModelV4Response;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResponse;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::Source;
use vercel_ai_provider::StreamError;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::generate_id;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::map_reasoning_to_provider_effort;
use vercel_ai_provider_utils::parse_tool_arguments_or_empty;
use vercel_ai_provider_utils::post_json_to_api_with_client_and_headers_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_and_headers_tapped;

use crate::convert_xai_chat_usage::XaiChatUsage;
use crate::convert_xai_chat_usage::convert_xai_chat_usage;
use crate::map_xai_finish_reason::map_xai_finish_reason;
use crate::supports_reasoning_effort::supports_reasoning_effort;
use crate::xai_config::XaiConfig;
use crate::xai_error::SERVICE_UNAVAILABLE_CODE;
use crate::xai_error::XaiFailedResponseHandler;

use super::convert_to_xai_chat_messages::convert_to_xai_chat_messages;
use super::xai_api_types::XaiChatChunk;
use super::xai_api_types::XaiChatResponse;
use super::xai_chat_options::XaiChatProviderOptions;
use super::xai_chat_options::extract_xai_chat_options;
use super::xai_prepare_tools::prepare_xai_tools;

/// xAI accepts any `https?://` image URL. Compiled once; `Regex` clones share
/// the underlying automaton, so `supported_urls()` stays cheap.
static IMAGE_URL_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^https?://.*$").ok());

/// Maps a provider-agnostic `ReasoningLevel` to xAI's `reasoning_effort`.
/// xAI has no `minimal`/`xhigh` tiers, so they fold into `low`/`high`. The
/// `off` level is handled separately (mapped to the literal `"none"`).
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

/// xAI (Grok) Chat Completions language model.
pub struct XaiChatLanguageModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiChatLanguageModel {
    /// Create a new xAI chat language model.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
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
        let xai_options = extract_xai_chat_options(&options.provider_options);

        // Parameters xAI's Chat Completions API does not support.
        if options.top_k.is_some() {
            warnings.push(Warning::unsupported("topK"));
        }
        if options.frequency_penalty.is_some() {
            warnings.push(Warning::unsupported("frequencyPenalty"));
        }
        if options.presence_penalty.is_some() {
            warnings.push(Warning::unsupported("presencePenalty"));
        }
        if options
            .stop_sequences
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        {
            warnings.push(Warning::unsupported("stopSequences"));
        }

        // Convert prompt to xAI messages.
        let (messages, msg_warnings) = convert_to_xai_chat_messages(&options.prompt)?;
        warnings.extend(msg_warnings);

        // Prepare tools.
        let prepared = prepare_xai_tools(&options.tools, &options.tool_choice);
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

        // Log-probabilities: `logprobs` is only ever `true` or absent.
        if xai_options.logprobs == Some(true) || xai_options.top_logprobs.is_some() {
            body["logprobs"] = Value::Bool(true);
        }
        if let Some(top_logprobs) = xai_options.top_logprobs {
            body["top_logprobs"] = json!(top_logprobs);
        }

        // Standardized generation settings.
        if let Some(max) = options.max_output_tokens {
            body["max_completion_tokens"] = json!(max);
        }
        set_optional_f32(&mut body, "temperature", options.temperature);
        set_optional_f32(&mut body, "top_p", options.top_p);
        if let Some(seed) = options.seed {
            body["seed"] = json!(seed);
        }

        // Reasoning effort (explicit option wins; otherwise mapped from the
        // top-level `ReasoningLevel`, gated by model support).
        if let Some(effort) = self.resolve_reasoning_effort(&xai_options, options, &mut warnings) {
            body["reasoning_effort"] = Value::String(effort);
        }

        // Parallel function calling.
        if let Some(parallel) = xai_options.parallel_function_calling {
            body["parallel_function_calling"] = Value::Bool(parallel);
        }

        // Response format. xAI always sends `strict: true` for json_schema.
        if let Some(ResponseFormat::Json { schema, name, .. }) = &options.response_format {
            body["response_format"] = match schema {
                Some(schema) => json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": name.as_deref().unwrap_or("response"),
                        "schema": schema,
                        "strict": true,
                    },
                }),
                None => json!({ "type": "json_object" }),
            };
        }

        Ok((body, warnings))
    }

    /// Resolve `reasoning_effort`. The explicit provider option wins; otherwise
    /// map a custom top-level `ReasoningLevel`, gated by model support, with
    /// `off` mapping to the literal `"none"`.
    fn resolve_reasoning_effort(
        &self,
        xai_options: &XaiChatProviderOptions,
        options: &LanguageModelV4CallOptions,
        warnings: &mut Vec<Warning>,
    ) -> Option<String> {
        if let Some(ref effort) = xai_options.reasoning_effort {
            return Some(effort.clone());
        }
        let level = options.reasoning?;
        if !is_custom_reasoning(Some(level)) {
            return None;
        }
        if !supports_reasoning_effort(&self.model_id) {
            warnings.push(Warning::unsupported_with_details(
                "reasoning",
                format!(
                    "reasoning \"{}\" is not supported by this model.",
                    level.as_str()
                ),
            ));
            return None;
        }
        if level == ReasoningLevel::Off {
            return Some("none".to_string());
        }
        map_reasoning_to_provider_effort(level, &REASONING_EFFORT_MAP, warnings)
    }
}

#[async_trait]
impl LanguageModelV4 for XaiChatLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        // xAI accepts image URLs: `{ 'image/*': [/^https?:\/\/.*$/] }`.
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

        let api_response = post_json_to_api_with_client_and_headers_tapped::<XaiChatResponse>(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            XaiFailedResponseHandler::new(),
            abort_signal,
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        let response = api_response.value;
        let response_headers = api_response.headers;

        // xAI can deliver a soft error with HTTP 200 in the `error` field.
        // Attach an `APICallError` cause (status 200, retryable iff the `code`
        // marks a transient outage) so the inference retry classifier treats it
        // like the TS `throw new APICallError({ statusCode: 200, isRetryable })`
        // — without the cause a transient 200-error degrades to non-retryable.
        if let Some(err) = &response.error {
            let retryable = response.code.as_deref() == Some(SERVICE_UNAVAILABLE_CODE);
            let response_body = serde_json::to_string(&response).unwrap_or_default();
            let api_err = vercel_ai_provider::APICallError::new(err.clone(), &url)
                .with_status(200)
                .with_retryable(retryable)
                .with_response_body(response_body);
            return Err(
                AISdkError::new(format!("xAI API error: {err}")).with_cause(Box::new(api_err))
            );
        }

        let choice = response
            .choices
            .as_ref()
            .and_then(|c| c.first())
            .ok_or_else(|| AISdkError::new("No choices in xAI response"))?;

        let mut content: Vec<AssistantContentPart> = Vec::new();

        // Text (before reasoning, matching TS order). Skip content that merely
        // echoes the last assistant message (an xAI quirk).
        if let Some(text) = &choice.message.content
            && !text.is_empty()
            && Some(text.as_str()) != last_assistant_text(&body)
        {
            content.push(AssistantContentPart::Text(TextPart::new(text.clone())));
        }

        // Reasoning.
        if let Some(reasoning) = &choice.message.reasoning_content
            && !reasoning.is_empty()
        {
            content.push(AssistantContentPart::Reasoning(ReasoningPart::new(
                reasoning.clone(),
            )));
        }

        // Tool calls.
        if let Some(tool_calls) = &choice.message.tool_calls {
            for tc in tool_calls {
                let input =
                    parse_tool_arguments_or_empty(&tc.function.arguments, &tc.function.name);
                content.push(AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    input,
                    provider_executed: None,
                    invalid: false,
                    invalid_reason: None,
                    provider_metadata: None,
                }));
            }
        }

        // Citations become URL sources.
        if let Some(citations) = &response.citations {
            for url in citations {
                content.push(AssistantContentPart::Source(Source::url(
                    generate_id("src"),
                    url.clone(),
                )));
            }
        }

        let finish_reason = map_xai_finish_reason(choice.finish_reason.as_deref());
        let usage = convert_xai_chat_usage(response.usage.as_ref());

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
        body["stream_options"] = json!({ "include_usage": true });

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

        // xAI can return HTTP 200 with a non-SSE JSON error body (content-type
        // `application/json` instead of `text/event-stream`). The SSE decoder
        // would find no `data:` lines and finish empty, hiding the error — so
        // detect it and surface it, mirroring the TS successful-response handler.
        if response_headers
            .get("content-type")
            .is_some_and(|ct| ct.contains("application/json"))
        {
            return Err(collect_json_stream_error(byte_stream, &url).await);
        }

        let request_body = body.clone();
        let last_assistant = last_assistant_text(&body).map(String::from);
        let stream = create_xai_chat_stream(byte_stream, warnings, include_raw, last_assistant);

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

/// Buffer a non-SSE JSON error body (an HTTP 200 with
/// `content-type: application/json`) and convert it to an `AISdkError` carrying
/// an `APICallError` cause so retry classification still applies. Mirrors the TS
/// stream handler's content-type branch.
async fn collect_json_stream_error(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    url: &str,
) -> AISdkError {
    use futures::StreamExt;
    let mut byte_stream = byte_stream;
    let mut buf = Vec::new();
    while let Some(chunk) = byte_stream.next().await {
        match chunk {
            Ok(bytes) => buf.extend_from_slice(&bytes),
            Err(e) => return AISdkError::new(format!("Stream read error: {e}")),
        }
    }
    let body = String::from_utf8_lossy(&buf).into_owned();
    // Mirror the TS stream handler: surface the raw `error` text (no `code`
    // prefix) and mark retryable only on an exact `code` match; an unparseable
    // body is a non-retryable "Invalid JSON response".
    let (message, retryable) = match serde_json::from_str::<crate::xai_error::XaiErrorData>(&body) {
        Ok(data) => (
            data.error_text().to_string(),
            data.code() == Some(SERVICE_UNAVAILABLE_CODE),
        ),
        Err(_) => ("Invalid JSON response".to_string(), false),
    };
    let api_err = vercel_ai_provider::APICallError::new(message.clone(), url)
        .with_status(200)
        .with_retryable(retryable)
        .with_response_body(body);
    AISdkError::new(format!("xAI API error: {message}")).with_cause(Box::new(api_err))
}

/// Return the last message's content string iff it is an assistant message with
/// a plain-string `content` — used to suppress duplicated echo content.
fn last_assistant_text(body: &Value) -> Option<&str> {
    let last = body.get("messages")?.as_array()?.last()?;
    if last.get("role")?.as_str()? != "assistant" {
        return None;
    }
    last.get("content")?.as_str()
}

/// Create a `LanguageModelV4StreamPart` stream from a raw xAI SSE byte stream.
fn create_xai_chat_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
    include_raw: bool,
    last_assistant_text: Option<String>,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    let stream = futures::stream::unfold(
        XaiStreamState::new(byte_stream, warnings, include_raw, last_assistant_text),
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
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            // A malformed / error chunk sets the raw finish reason
                            // to "error", which maps to `Other` — matching the TS
                            // xAI reference (unified `other`) and the coco provider
                            // majority (openai / openai-compatible / google /
                            // anthropic). The emitted `Error` stream part is the
                            // real error signal, not the finish reason.
                            state.pending.push_back(LanguageModelV4StreamPart::Finish {
                                usage: convert_xai_chat_usage(state.usage.as_ref()),
                                finish_reason: map_xai_finish_reason(
                                    state.finish_reason.as_deref(),
                                ),
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

struct XaiStreamState {
    byte_stream: vercel_ai_provider_utils::ByteStream,
    decoder: vercel_ai_provider_utils::SseDecoder,
    pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
    text_started: bool,
    text_id: String,
    reasoning_started: bool,
    reasoning_id: String,
    /// Last reasoning delta, used to drop exact-duplicate consecutive deltas.
    last_reasoning_delta: Option<String>,
    /// The last assistant message's content, echoed content is suppressed.
    last_assistant_text: Option<String>,
    usage: Option<XaiChatUsage>,
    /// Raw `finish_reason`; a malformed / error chunk sets it to `"error"`
    /// (maps to `Other`), mirroring openai-compatible.
    finish_reason: Option<String>,
    finish_emitted: bool,
    done: bool,
    metadata_emitted: bool,
    include_raw: bool,
}

impl XaiStreamState {
    fn new(
        byte_stream: vercel_ai_provider_utils::ByteStream,
        warnings: Vec<Warning>,
        include_raw: bool,
        last_assistant_text: Option<String>,
    ) -> Self {
        let mut pending = std::collections::VecDeque::new();
        pending.push_back(LanguageModelV4StreamPart::StreamStart { warnings });
        Self {
            byte_stream,
            decoder: vercel_ai_provider_utils::SseDecoder::new(),
            pending,
            text_started: false,
            text_id: "txt-0".to_string(),
            reasoning_started: false,
            reasoning_id: "reasoning-0".to_string(),
            last_reasoning_delta: None,
            last_assistant_text,
            usage: None,
            finish_reason: None,
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

    fn process_data_line(&mut self, data: &str) {
        let raw: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                self.finish_reason = Some("error".to_string());
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: StreamError::new(format!("Failed to parse xAI chunk: {e}")),
                });
                return;
            }
        };

        if self.include_raw {
            self.pending.push_back(LanguageModelV4StreamPart::Raw {
                raw_value: raw.clone(),
            });
        }

        // A soft error may arrive as `{ "error": ... }` (string or object).
        if let Some(error) = raw.get("error") {
            let message = error
                .as_str()
                .map(String::from)
                .or_else(|| {
                    error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "Unknown error".to_string());
            self.finish_reason = Some("error".to_string());
            self.pending.push_back(LanguageModelV4StreamPart::Error {
                error: StreamError::new(message),
            });
            return;
        }

        let chunk: XaiChatChunk = match serde_json::from_value(raw) {
            Ok(c) => c,
            Err(e) => {
                self.finish_reason = Some("error".to_string());
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: StreamError::new(format!("Invalid xAI chunk structure: {e}")),
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

        // Citations (typically arrive in the final chunk) become URL sources.
        if let Some(citations) = &chunk.citations {
            for url in citations {
                self.pending
                    .push_back(LanguageModelV4StreamPart::Source(Source::url(
                        generate_id("src"),
                        url.clone(),
                    )));
            }
        }

        // xAI reports usage at the top level.
        if let Some(u) = chunk.usage {
            self.usage = Some(u);
        }

        let Some(choices) = chunk.choices else {
            return;
        };
        for choice in choices {
            if let Some(fr) = choice.finish_reason {
                self.finish_reason = Some(fr);
            }
            let Some(delta) = choice.delta else {
                continue;
            };

            // Reasoning (dropping exact-duplicate consecutive deltas).
            if let Some(reasoning) = delta.reasoning_content
                && !reasoning.is_empty()
                && self.last_reasoning_delta.as_deref() != Some(reasoning.as_str())
            {
                self.last_reasoning_delta = Some(reasoning.clone());
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
                        delta: reasoning,
                        provider_metadata: None,
                    });
            }

            // Text ends any active reasoning block first (matching TS, which
            // closes reasoning before the echo-dup check), then emits unless the
            // content merely echoes the last assistant message.
            if let Some(content) = delta.content
                && !content.is_empty()
            {
                self.close_reasoning();
                if self.last_assistant_text.as_deref() != Some(content.as_str()) {
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
                            delta: content,
                            provider_metadata: None,
                        });
                }
            }

            // Tool calls arrive complete in a single delta (id + name + full
            // arguments), so each is emitted as start → delta → end → call.
            if let Some(tool_calls) = delta.tool_calls {
                self.close_reasoning();
                for tc in tool_calls {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: tc.id.clone(),
                            tool_name: tc.function.name.clone(),
                            provider_executed: None,
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id: tc.id.clone(),
                            delta: tc.function.arguments.clone(),
                            provider_metadata: None,
                        });
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                            id: tc.id.clone(),
                            provider_metadata: None,
                        });
                    self.pending.push_back(LanguageModelV4StreamPart::ToolCall(
                        LanguageModelV4ToolCall::new(
                            tc.id,
                            tc.function.name,
                            tc.function.arguments,
                        ),
                    ));
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
#[path = "xai_chat_language_model.test.rs"]
mod tests;
