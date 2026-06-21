use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
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
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client;
use vercel_ai_provider_utils::post_stream_to_api_with_client;

use crate::openai_config::OpenAIConfig;
use crate::openai_error::OpenAIFailedResponseHandler;

use super::convert_completion_usage::convert_openai_completion_usage;
use super::convert_to_completion_prompt::convert_to_completion_prompt;
use super::map_finish_reason::map_openai_completion_finish_reason;
use super::openai_completion_api::OpenAICompletionChunk;
use super::openai_completion_api::OpenAICompletionResponse;
use super::openai_completion_options::extract_completion_options;

/// Default text content-part ID for completion responses.
const COMPLETION_TEXT_ID: &str = "0";

/// OpenAI legacy Completions language model.
pub struct OpenAICompletionLanguageModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAICompletionLanguageModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

/// Collect warnings for features unsupported by the legacy Completions API.
fn collect_completion_warnings(options: &LanguageModelV4CallOptions) -> Vec<Warning> {
    let mut warnings = Vec::new();

    if options.top_k.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "topK".into(),
            details: None,
        });
    }

    if let Some(ref fmt) = options.response_format {
        match fmt {
            ResponseFormat::Text => {} // no warning for text
            _ => {
                warnings.push(Warning::Unsupported {
                    feature: "responseFormat".into(),
                    details: Some(
                        "JSON response format is not supported by the completion API.".into(),
                    ),
                });
            }
        }
    }

    if options.tools.as_ref().is_some_and(|t| !t.is_empty()) {
        warnings.push(Warning::Unsupported {
            feature: "tools".into(),
            details: None,
        });
    }

    if options.tool_choice.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "toolChoice".into(),
            details: None,
        });
    }

    warnings
}

/// Build the request body shared between `do_generate` and `do_stream`.
fn build_completion_body(
    model_id: &str,
    prompt: &str,
    options: &LanguageModelV4CallOptions,
    openai_opts: &super::openai_completion_options::OpenAICompletionProviderOptions,
    auto_stop: &[String],
    stream: bool,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "prompt": prompt,
    });

    if stream {
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
    }

    if let Some(max) = options.max_output_tokens {
        body["max_tokens"] = json!(max);
    }
    if let Some(temp) = options.temperature {
        body["temperature"] = json!(temp);
    }
    if let Some(top_p) = options.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(fp) = options.frequency_penalty {
        body["frequency_penalty"] = json!(fp);
    }
    if let Some(pp) = options.presence_penalty {
        body["presence_penalty"] = json!(pp);
    }
    if let Some(seed) = options.seed {
        body["seed"] = json!(seed);
    }

    // Merge auto-generated stop sequences with user-provided ones.
    let mut stop: Vec<String> = auto_stop.to_vec();
    if let Some(ref user_stop) = options.stop_sequences {
        stop.extend(user_stop.iter().cloned());
    }
    if !stop.is_empty() {
        body["stop"] = json!(stop);
    }

    if let Some(echo) = openai_opts.echo {
        body["echo"] = Value::Bool(echo);
    }
    if let Some(ref bias) = openai_opts.logit_bias {
        body["logit_bias"] = serde_json::to_value(bias).unwrap_or_default();
    }
    set_completion_logprobs(&mut body, openai_opts);
    if let Some(ref suffix) = openai_opts.suffix {
        body["suffix"] = Value::String(suffix.clone());
    }
    if let Some(ref user) = openai_opts.user {
        body["user"] = Value::String(user.clone());
    }

    body
}

/// Merge config headers with call-level headers (call-level takes precedence).
fn merge_headers(
    config_headers: std::collections::HashMap<String, String>,
    call_headers: &Option<std::collections::HashMap<String, String>>,
) -> std::collections::HashMap<String, String> {
    let mut headers = config_headers;
    if let Some(extra) = call_headers {
        for (k, v) in extra {
            headers.insert(k.clone(), v.clone());
        }
    }
    headers
}

/// Build provider metadata — always includes `{ "openai": {} }`, conditionally
/// populated with logprobs when present.
fn build_completion_provider_metadata(response: &OpenAICompletionResponse) -> ProviderMetadata {
    let mut openai_obj = serde_json::Map::new();
    if let Some(logprobs) = response.choices.first().and_then(|c| c.logprobs.as_ref()) {
        openai_obj.insert("logprobs".to_string(), logprobs.clone());
    }
    let mut meta = ProviderMetadata::default();
    meta.0
        .insert("openai".to_string(), serde_json::Value::Object(openai_obj));
    meta
}

#[async_trait]
impl LanguageModelV4 for OpenAICompletionLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let openai_opts = extract_completion_options(&options.provider_options);
        let conversion = convert_to_completion_prompt(&options.prompt)?;
        let warnings = collect_completion_warnings(options);

        let body = build_completion_body(
            &self.model_id,
            &conversion.prompt,
            options,
            &openai_opts,
            &conversion.stop_sequences,
            false,
        );

        let url = self.config.url("/completions");
        let headers = merge_headers(self.config.get_headers(), &options.headers);

        let response: OpenAICompletionResponse = post_json_to_api_with_client(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            OpenAIFailedResponseHandler,
            abort_signal,
            self.config.client.clone(),
        )
        .await?;

        let text = response
            .choices
            .first()
            .and_then(|c| c.text.clone())
            .unwrap_or_default();

        let finish_reason = map_openai_completion_finish_reason(
            response
                .choices
                .first()
                .and_then(|c| c.finish_reason.as_deref()),
        );
        let usage = convert_openai_completion_usage(response.usage.as_ref());
        let provider_metadata = build_completion_provider_metadata(&response);

        let timestamp = response
            .created
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0));

        Ok(LanguageModelV4GenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text,
                provider_metadata: None,
            })],
            usage,
            finish_reason,
            warnings,
            provider_metadata: Some(provider_metadata),
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
        let openai_opts = extract_completion_options(&options.provider_options);
        let conversion = convert_to_completion_prompt(&options.prompt)?;
        let warnings = collect_completion_warnings(options);

        let body = build_completion_body(
            &self.model_id,
            &conversion.prompt,
            options,
            &openai_opts,
            &conversion.stop_sequences,
            true,
        );

        let url = self.config.url("/completions");
        let headers = merge_headers(self.config.get_headers(), &options.headers);

        let byte_stream = post_stream_to_api_with_client(
            &url,
            Some(headers),
            &body,
            abort_signal,
            self.config.client.clone(),
        )
        .await?;

        let request_body = body.clone();
        let stream = create_completion_stream(byte_stream, warnings);

        Ok(LanguageModelV4StreamResult {
            stream,
            request: Some(LanguageModelV4Request {
                body: Some(request_body),
            }),
            response: Some(LanguageModelV4StreamResponse::new()),
        })
    }
}

fn create_completion_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    use futures::StreamExt;

    struct State {
        byte_stream: vercel_ai_provider_utils::ByteStream,
        buffer: String,
        pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
        is_first_chunk: bool,
        text_started: bool,
        text_id: String,
        usage: Option<super::openai_completion_api::OpenAICompletionUsage>,
        logprobs: Option<serde_json::Value>,
        finish_reason: Option<String>,
        done: bool,
        finish_emitted: bool,
    }

    let stream = futures::stream::unfold(
        State {
            byte_stream,
            buffer: String::new(),
            pending: {
                let mut q = std::collections::VecDeque::new();
                q.push_back(LanguageModelV4StreamPart::StreamStart { warnings });
                q
            },
            is_first_chunk: true,
            text_started: false,
            text_id: COMPLETION_TEXT_ID.to_string(),
            usage: None,
            logprobs: None,
            finish_reason: None,
            done: false,
            finish_emitted: false,
        },
        |mut state| async move {
            loop {
                if let Some(event) = state.pending.pop_front() {
                    return Some((Ok(event), state));
                }
                if state.done {
                    return None;
                }
                match state.byte_stream.next().await {
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        state.buffer.push_str(&text);

                        // Process lines
                        while let Some(pos) = state.buffer.find('\n') {
                            let line = state.buffer[..pos].trim_end_matches('\r').to_string();
                            state.buffer = state.buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    continue;
                                }
                                match serde_json::from_str::<OpenAICompletionChunk>(data) {
                                    Ok(chunk) => {
                                        // Emit ResponseMetadata on first parsed chunk.
                                        if state.is_first_chunk {
                                            state.is_first_chunk = false;
                                            let timestamp = chunk
                                                .created
                                                .and_then(|ts| {
                                                    chrono::DateTime::from_timestamp(ts as i64, 0)
                                                })
                                                .map(|dt| dt.to_rfc3339());
                                            let mut meta_builder =
                                                vercel_ai_provider::ResponseMetadata::new();
                                            if let Some(ref id) = chunk.id {
                                                meta_builder = meta_builder.with_id(id.clone());
                                            }
                                            if let Some(ref model) = chunk.model {
                                                meta_builder =
                                                    meta_builder.with_model(model.clone());
                                            }
                                            if let Some(ts) = timestamp {
                                                meta_builder = meta_builder.with_timestamp(ts);
                                            }
                                            state.pending.push_back(
                                                LanguageModelV4StreamPart::ResponseMetadata(
                                                    meta_builder,
                                                ),
                                            );
                                        }

                                        if let Some(ref u) = chunk.usage {
                                            state.usage = Some(u.clone());
                                        }
                                        if let Some(ref choices) = chunk.choices {
                                            for choice in choices {
                                                if let Some(ref fr) = choice.finish_reason {
                                                    state.finish_reason = Some(fr.clone());
                                                }
                                                // Accumulate logprobs from chunks.
                                                if let Some(ref lp) = choice.logprobs {
                                                    state.logprobs = Some(lp.clone());
                                                }
                                                if let Some(ref text) = choice.text
                                                    && !text.is_empty()
                                                {
                                                    if !state.text_started {
                                                        state.text_started = true;
                                                        state.pending.push_back(
                                                            LanguageModelV4StreamPart::TextStart {
                                                                id: state.text_id.clone(),
                                                                provider_metadata: None,
                                                            },
                                                        );
                                                    }
                                                    state.pending.push_back(
                                                        LanguageModelV4StreamPart::TextDelta {
                                                            id: state.text_id.clone(),
                                                            delta: text.clone(),
                                                            provider_metadata: None,
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        // Emit error for failed chunk parsing.
                                        state.pending.push_back(LanguageModelV4StreamPart::Error {
                                            error: vercel_ai_provider::StreamError::new(format!(
                                                "Failed to parse completion chunk: {data}"
                                            )),
                                        });
                                    }
                                }
                            }
                        }

                        if !state.pending.is_empty() {
                            // Yield events
                        } else {
                            continue;
                        }
                    }
                    Some(Err(e)) => {
                        state.done = true;
                        return Some((
                            Err(AISdkError::new(format!("Stream read error: {e}"))),
                            state,
                        ));
                    }
                    None => {
                        state.done = true;
                        if state.text_started {
                            state.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                                id: state.text_id.clone(),
                                provider_metadata: None,
                            });
                        }
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            // Always create provider_metadata with openai key.
                            let mut openai_obj = serde_json::Map::new();
                            if let Some(ref logprobs) = state.logprobs {
                                openai_obj.insert("logprobs".to_string(), logprobs.clone());
                            }
                            let mut meta = ProviderMetadata::default();
                            meta.0.insert(
                                "openai".to_string(),
                                serde_json::Value::Object(openai_obj),
                            );
                            state.pending.push_back(LanguageModelV4StreamPart::Finish {
                                usage: convert_openai_completion_usage(state.usage.as_ref()),
                                finish_reason: map_openai_completion_finish_reason(
                                    state.finish_reason.as_deref(),
                                ),
                                provider_metadata: Some(meta),
                            });
                        }
                        if let Some(event) = state.pending.pop_front() {
                            return Some((Ok(event), state));
                        }
                        return None;
                    }
                }
            }
        },
    );

    Box::pin(stream)
}

/// Set logprobs on the request body, matching the TS behavior:
/// - `true` → `logprobs: 0`
/// - `false` → omit
/// - number → `logprobs: <number>`
fn set_completion_logprobs(
    body: &mut Value,
    options: &super::openai_completion_options::OpenAICompletionProviderOptions,
) {
    if let Some(ref logprobs) = options.logprobs {
        match logprobs {
            Value::Bool(true) => {
                body["logprobs"] = json!(0);
            }
            Value::Bool(false) => {}
            Value::Number(n) => {
                body["logprobs"] = Value::Number(n.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "openai_completion_language_model.test.rs"]
mod tests;
