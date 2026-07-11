use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Request;
use vercel_ai_provider::LanguageModelV4Response;
use vercel_ai_provider::LanguageModelV4StreamResponse;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::Source;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::generate_id;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::map_reasoning_to_provider_effort;
use vercel_ai_provider_utils::parse_tool_arguments_or_empty;
use vercel_ai_provider_utils::post_json_to_api_with_client_and_headers_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_and_headers_tapped;

use crate::supports_reasoning_effort::supports_reasoning_effort;
use crate::xai_config::XaiConfig;
use crate::xai_error::SERVICE_UNAVAILABLE_CODE;
use crate::xai_error::XaiFailedResponseHandler;

use super::convert_to_xai_responses_input::convert_to_xai_responses_input;
use super::convert_xai_responses_usage::convert_xai_responses_usage;
use super::map_xai_responses_finish_reason::map_xai_responses_finish_reason;
use super::xai_responses_api_types::ResponseMessageContentPart;
use super::xai_responses_api_types::ResponseOutputItem;
use super::xai_responses_api_types::ResponsesToolNames;
use super::xai_responses_api_types::XaiResponsesResponse;
use super::xai_responses_api_types::resolve_server_tool;
use super::xai_responses_options::XaiResponsesProviderOptions;
use super::xai_responses_options::extract_xai_responses_options;
use super::xai_responses_prepare_tools::prepare_responses_tools;
use super::xai_responses_stream::create_xai_responses_stream;

/// xAI accepts any `https?://` URL for images / documents.
static URL_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^https?://.*$").ok());

/// Maps a provider-agnostic `ReasoningLevel` to xAI's `reasoning.effort`.
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

/// xAI (Grok) Responses API language model.
///
/// Opt-in surface: the runtime routes to Chat Completions by default; this
/// model is reached via `XaiProvider::responses`. The wire format mirrors the
/// OpenAI Responses API (typed `input` items, `output` items, and
/// `response.*` SSE events).
pub struct XaiResponsesLanguageModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiResponsesLanguageModel {
    /// Create a new xAI Responses language model.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Build the request body, collect warnings, and resolve the caller's
    /// provider-tool names. Public so cross-crate tests can inspect the wire
    /// shape without dispatching HTTP.
    pub fn get_args(
        &self,
        options: &LanguageModelV4CallOptions,
    ) -> Result<(Value, Vec<Warning>, ResponsesToolNames), AISdkError> {
        let mut warnings = Vec::new();
        let xai_options = extract_xai_responses_options(&options.provider_options);

        if options
            .stop_sequences
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        {
            warnings.push(Warning::unsupported("stopSequences"));
        }

        let tool_names = resolve_tool_names(&options.tools);

        let (input, input_warnings) = convert_to_xai_responses_input(&options.prompt)?;
        warnings.extend(input_warnings);

        let prepared = prepare_responses_tools(&options.tools, &options.tool_choice);
        warnings.extend(prepared.warnings);

        // `include`: start from the option, then force reasoning encrypted
        // content when the response is not stored (needed to round-trip
        // reasoning tokens under Zero Data Retention).
        let store = xai_options.store.unwrap_or(true);
        let mut include: Option<Vec<String>> = xai_options.include.clone();
        if !store {
            let list = include.get_or_insert_with(Vec::new);
            if !list.iter().any(|v| v == "reasoning.encrypted_content") {
                list.push("reasoning.encrypted_content".to_string());
            }
        }

        let mut body = json!({
            "model": self.model_id,
            "input": input,
        });

        if xai_options.logprobs == Some(true) || xai_options.top_logprobs.is_some() {
            body["logprobs"] = Value::Bool(true);
        }
        if let Some(top_logprobs) = xai_options.top_logprobs {
            body["top_logprobs"] = json!(top_logprobs);
        }
        if let Some(max) = options.max_output_tokens {
            body["max_output_tokens"] = json!(max);
        }
        set_optional_f32(&mut body, "temperature", options.temperature);
        set_optional_f32(&mut body, "top_p", options.top_p);
        if let Some(seed) = options.seed {
            body["seed"] = json!(seed);
        }

        // Response format → `text.format`. xAI always sends `strict: true` for
        // json_schema.
        if let Some(ResponseFormat::Json {
            schema,
            name,
            description,
        }) = &options.response_format
        {
            let format = match schema {
                Some(schema) => {
                    let mut fmt = json!({
                        "type": "json_schema",
                        "strict": true,
                        "name": name.as_deref().unwrap_or("response"),
                        "schema": schema,
                    });
                    if let Some(desc) = description {
                        fmt["description"] = Value::String(desc.clone());
                    }
                    fmt
                }
                None => json!({ "type": "json_object" }),
            };
            body["text"] = json!({ "format": format });
        }

        // Reasoning: `{ effort?, summary? }`.
        let effort = self.resolve_reasoning_effort(&xai_options, options, &mut warnings);
        if effort.is_some() || xai_options.reasoning_summary.is_some() {
            let mut reasoning = serde_json::Map::new();
            if let Some(effort) = effort {
                reasoning.insert("effort".into(), Value::String(effort));
            }
            if let Some(ref summary) = xai_options.reasoning_summary {
                reasoning.insert("summary".into(), Value::String(summary.clone()));
            }
            body["reasoning"] = Value::Object(reasoning);
        }

        // `store` is only sent when explicitly disabled (server default is true).
        if xai_options.store == Some(false) {
            body["store"] = Value::Bool(false);
        }
        if let Some(include) = include {
            body["include"] = json!(include);
        }
        if let Some(ref prev) = xai_options.previous_response_id {
            body["previous_response_id"] = Value::String(prev.clone());
        }

        if !prepared.tools.is_empty() {
            body["tools"] = Value::Array(prepared.tools);
        }
        if let Some(tc) = prepared.tool_choice {
            body["tool_choice"] = tc;
        }

        Ok((body, warnings, tool_names))
    }

    /// Resolve `reasoning.effort`. The explicit provider option wins; otherwise
    /// map a custom top-level `ReasoningLevel`, gated by model support, with
    /// `off` mapping to the literal `"none"`.
    fn resolve_reasoning_effort(
        &self,
        xai_options: &XaiResponsesProviderOptions,
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
impl LanguageModelV4 for XaiResponsesLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        // xAI Responses accepts image URLs and non-image documents (PDF, text)
        // by URL: `{ 'image/*' | 'application/pdf' | 'text/*': [/^https?/] }`.
        let mut map = HashMap::new();
        if let Some(re) = URL_RE.as_ref() {
            map.insert("image/*".to_string(), vec![re.clone()]);
            map.insert("application/pdf".to_string(), vec![re.clone()]);
            map.insert("text/*".to_string(), vec![re.clone()]);
        }
        map
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let (body, warnings, tool_names) = self.get_args(options)?;
        let url = self.config.url("/responses");
        let headers = self.config.get_headers();

        let api_response = post_json_to_api_with_client_and_headers_tapped::<XaiResponsesResponse>(
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

        // xAI can deliver a soft error with HTTP 200 in the `{code, error}`
        // shape. Attach an `APICallError` cause (status 200, retryable iff the
        // `code` marks a transient outage) so the inference retry classifier
        // treats it like the TS `throw new APICallError({ statusCode: 200 })`.
        if let Some(err) = &response.error {
            let retryable = response.code.as_deref() == Some(SERVICE_UNAVAILABLE_CODE);
            let response_body = json!({ "code": response.code, "error": err }).to_string();
            let api_err = APICallError::new(err.clone(), &url)
                .with_status(200)
                .with_retryable(retryable)
                .with_response_body(response_body);
            return Err(
                AISdkError::new(format!("xAI API error: {err}")).with_cause(Box::new(api_err))
            );
        }

        let mut content: Vec<AssistantContentPart> = Vec::new();
        let mut has_function_call = false;

        for part in &response.output {
            build_generate_content(part, &tool_names, &mut content, &mut has_function_call);
        }

        let unified = if has_function_call {
            vercel_ai_provider::UnifiedFinishReason::ToolUse
        } else {
            map_xai_responses_finish_reason(response.status.as_deref())
        };
        let finish_reason = vercel_ai_provider::FinishReason {
            unified,
            raw: response.status.clone(),
        };

        let usage = response
            .usage
            .as_ref()
            .map(convert_xai_responses_usage)
            .unwrap_or_default();

        let provider_metadata = response
            .usage
            .as_ref()
            .and_then(|u| u.cost_in_usd_ticks)
            .map(cost_provider_metadata);

        let timestamp = response
            .created_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

        Ok(LanguageModelV4GenerateResult {
            content,
            usage,
            finish_reason,
            warnings,
            provider_metadata,
            request: Some(LanguageModelV4Request {
                body: Some(body.clone()),
            }),
            response: Some(LanguageModelV4Response {
                id: response.id.clone(),
                timestamp,
                model_id: response.model.clone(),
                headers: Some(response_headers),
                body: serde_json::to_value(&response).ok(),
            }),
        })
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        let (mut body, warnings, tool_names) = self.get_args(options)?;
        body["stream"] = Value::Bool(true);

        let include_raw = options.include_raw_chunks.unwrap_or(false);
        let url = self.config.url("/responses");
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

        // A non-SSE JSON error body (HTTP 200, `content-type: application/json`)
        // would decode to an empty stream, hiding the error — detect and surface
        // it, mirroring the chat model's successful-response handler.
        if response_headers
            .get("content-type")
            .is_some_and(|ct| ct.contains("application/json"))
        {
            return Err(collect_json_stream_error(byte_stream, &url).await);
        }

        let request_body = body.clone();
        let stream = create_xai_responses_stream(byte_stream, warnings, include_raw, tool_names);

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

/// Build the `providerMetadata.xai.costInUsdTicks` metadata.
pub(super) fn cost_provider_metadata(cost: f64) -> ProviderMetadata {
    let mut pm = ProviderMetadata::new();
    pm.set("xai", json!({ "costInUsdTicks": cost }));
    pm
}

/// Append the content parts produced by a single `do_generate` output item.
fn build_generate_content(
    part: &ResponseOutputItem,
    tool_names: &ResponsesToolNames,
    content: &mut Vec<AssistantContentPart>,
    has_function_call: &mut bool,
) {
    if let ResponseOutputItem::FileSearchCall {
        id,
        queries,
        results,
        ..
    } = part
    {
        let tool_name = tool_names
            .file_search
            .clone()
            .unwrap_or_else(|| "file_search".to_string());
        content.push(AssistantContentPart::ToolCall(
            ToolCallPart::new(id.clone(), tool_name.clone(), json!({}))
                .with_provider_executed(true),
        ));
        content.push(AssistantContentPart::ToolResult(ToolResultPart::new(
            id.clone(),
            tool_name,
            ToolResultContent::json(file_search_result(queries.as_deref(), results.as_deref())),
        )));
        return;
    }

    if let Some((kind, item)) = part.as_server_tool() {
        let (tool_name, input) = resolve_server_tool(kind, item, tool_names);
        let parsed = parse_tool_arguments_or_empty(&input, &tool_name);
        content.push(AssistantContentPart::ToolCall(
            ToolCallPart::new(item.id.clone(), tool_name, parsed).with_provider_executed(true),
        ));
        return;
    }

    match part {
        ResponseOutputItem::Message { content: parts, .. } => {
            for cp in parts {
                push_message_content(cp, content);
            }
        }
        ResponseOutputItem::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        } => {
            *has_function_call = true;
            let parsed = parse_tool_arguments_or_empty(arguments, name);
            content.push(AssistantContentPart::ToolCall(ToolCallPart::new(
                call_id.clone(),
                name.clone(),
                parsed,
            )));
        }
        ResponseOutputItem::Reasoning {
            id,
            summary,
            content: reasoning_content,
            encrypted_content,
        } => {
            let reasoning_text = reasoning_text(summary, reasoning_content.as_deref());
            if !reasoning_text.is_empty() || encrypted_content.is_some() {
                let mut rp = ReasoningPart::new(reasoning_text);
                if encrypted_content.is_some() || id.is_some() {
                    let mut xai = serde_json::Map::new();
                    if let Some(enc) = encrypted_content {
                        xai.insert(
                            "reasoningEncryptedContent".into(),
                            Value::String(enc.clone()),
                        );
                    }
                    if let Some(id) = id {
                        xai.insert("itemId".into(), Value::String(id.clone()));
                    }
                    let mut pm = ProviderMetadata::new();
                    pm.set("xai", Value::Object(xai));
                    rp = rp.with_metadata(pm);
                }
                content.push(AssistantContentPart::Reasoning(rp));
            }
        }
        _ => {}
    }
}

/// Emit text + url_citation sources for one message content part.
fn push_message_content(cp: &ResponseMessageContentPart, content: &mut Vec<AssistantContentPart>) {
    if let Some(text) = &cp.text
        && !text.is_empty()
    {
        content.push(AssistantContentPart::Text(TextPart::new(text.clone())));
    }
    if let Some(annotations) = &cp.annotations {
        for ann in annotations {
            if let Some((url, title)) = ann.url_citation() {
                let mut src = Source::url(generate_id("src"), url.to_string());
                src.title = Some(title.unwrap_or(url).to_string());
                content.push(AssistantContentPart::Source(src));
            }
        }
    }
}

/// Join the reasoning text, preferring the condensed summary over the raw
/// `content` channel (matching the TS).
pub(super) fn reasoning_text(
    summary: &[super::xai_responses_api_types::ReasoningSummaryPart],
    content: Option<&[super::xai_responses_api_types::ReasoningTextPart]>,
) -> String {
    if !summary.is_empty() {
        summary
            .iter()
            .map(|s| s.text.as_str())
            .filter(|t| !t.is_empty())
            .collect()
    } else {
        content
            .unwrap_or(&[])
            .iter()
            .map(|c| c.text.as_str())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Build the `file_search` tool-result payload.
pub(super) fn file_search_result(
    queries: Option<&[String]>,
    results: Option<&[super::xai_responses_api_types::FileSearchResult]>,
) -> Value {
    let results_json = results.map(|rs| {
        rs.iter()
            .map(|r| {
                json!({
                    "fileId": r.file_id,
                    "filename": r.filename,
                    "score": r.score,
                    "text": r.text,
                })
            })
            .collect::<Vec<_>>()
    });
    json!({
        "queries": queries.unwrap_or(&[]),
        "results": results_json,
    })
}

/// Buffer a non-SSE JSON error body and convert it to an `AISdkError` carrying
/// an `APICallError` cause. Mirrors the chat model's stream handler.
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
    let (message, retryable) = match serde_json::from_str::<crate::xai_error::XaiErrorData>(&body) {
        Ok(data) => (
            data.error_text().to_string(),
            data.code() == Some(SERVICE_UNAVAILABLE_CODE),
        ),
        Err(_) => ("Invalid JSON response".to_string(), false),
    };
    let api_err = APICallError::new(message.clone(), url)
        .with_status(200)
        .with_retryable(retryable)
        .with_response_body(body);
    AISdkError::new(format!("xAI API error: {message}")).with_cause(Box::new(api_err))
}

/// Collect the resolved provider-tool names declared on the request.
fn resolve_tool_names(tools: &Option<Vec<LanguageModelV4Tool>>) -> ResponsesToolNames {
    let mut names = ResponsesToolNames::default();
    let Some(tools) = tools else {
        return names;
    };
    for tool in tools {
        if let LanguageModelV4Tool::Provider(pt) = tool {
            match pt.id.as_str() {
                "xai.web_search" => names.web_search = Some(pt.name.clone()),
                "xai.x_search" => names.x_search = Some(pt.name.clone()),
                "xai.code_execution" => names.code_execution = Some(pt.name.clone()),
                "xai.mcp" => names.mcp = Some(pt.name.clone()),
                "xai.file_search" => names.file_search = Some(pt.name.clone()),
                _ => {}
            }
        }
    }
    names
}

fn set_optional_f32(body: &mut Value, key: &str, value: Option<f32>) {
    if let Some(v) = value {
        body[key] = json!(v);
    }
}

#[cfg(test)]
#[path = "xai_responses_language_model.test.rs"]
mod tests;
