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
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::StreamError;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client_and_headers;
use vercel_ai_provider_utils::post_stream_to_api_with_client_and_headers;

use crate::openai_compatible_config::OpenAICompatibleConfig;
use crate::provider_options_key::warn_if_deprecated_provider_options_key;

use super::convert_completion_usage::convert_openai_compatible_completion_usage;
use super::convert_to_completion_prompt::convert_to_completion_prompt;
use super::map_finish_reason::map_openai_compatible_completion_finish_reason;
use super::openai_compatible_completion_api::OpenAICompatibleCompletionChunk;
use super::openai_compatible_completion_api::OpenAICompatibleCompletionResponse;
use super::openai_compatible_completion_options::extract_completion_options;

/// OpenAI-compatible legacy Completions language model.
pub struct OpenAICompatibleCompletionLanguageModel {
    model_id: String,
    config: Arc<OpenAICompatibleConfig>,
}

impl OpenAICompatibleCompletionLanguageModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAICompatibleConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl LanguageModelV4 for OpenAICompatibleCompletionLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        self.config
            .supported_urls
            .as_ref()
            .map(|f| f())
            .unwrap_or_default()
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let mut warnings = Vec::new();
        let provider_name = self.config.provider_options_name();
        warn_if_deprecated_provider_options_key(
            provider_name,
            options.provider_options.as_ref(),
            &mut warnings,
        );
        let (compat_opts, passthrough) =
            extract_completion_options(&options.provider_options, provider_name);
        let prompt_result = convert_to_completion_prompt(&options.prompt)?;

        // Warn about unsupported features
        if options.top_k.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "topK".into(),
                details: Some("This model does not support topK. topK is ignored.".into()),
            });
        }
        if options.tools.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "tools".into(),
                details: Some("This model does not support tools.".into()),
            });
        }
        if options.tool_choice.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "toolChoice".into(),
                details: Some("This model does not support toolChoice.".into()),
            });
        }
        if let Some(ref fmt) = options.response_format
            && !matches!(fmt, vercel_ai_provider::ResponseFormat::Text)
        {
            warnings.push(Warning::Unsupported {
                feature: "responseFormat".into(),
                details: Some("This model does not support non-text response formats.".into()),
            });
        }

        let mut body = json!({
            "model": self.model_id,
            "prompt": prompt_result.prompt,
        });

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

        // Merge stop sequences: user-provided + prompt-generated
        let mut all_stop = prompt_result.stop_sequences;
        if let Some(ref user_stop) = options.stop_sequences {
            all_stop.extend(user_stop.iter().cloned());
        }
        if !all_stop.is_empty() {
            body["stop"] = json!(all_stop);
        }

        if let Some(echo) = compat_opts.echo {
            body["echo"] = Value::Bool(echo);
        }
        if let Some(ref bias) = compat_opts.logit_bias {
            body["logit_bias"] = serde_json::to_value(bias).unwrap_or_default();
        }
        if let Some(ref suffix) = compat_opts.suffix {
            body["suffix"] = Value::String(suffix.clone());
        }
        if let Some(ref user) = compat_opts.user {
            body["user"] = Value::String(user.clone());
        }

        // Deep-merge extra_body onto the wire body. Callers own
        // wire-correct nesting; deep merge places nested overlays
        // at the right slot without clobbering sibling typed writes.
        if !passthrough.is_empty() {
            let overlay = Value::Object(passthrough.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // Apply request body transform
        let body = self.config.transform_body(body);

        let url = self.config.url("/completions");
        let headers = self.config.get_headers();

        let api_response =
            post_json_to_api_with_client_and_headers::<OpenAICompatibleCompletionResponse>(
                &url,
                Some(headers),
                &body,
                JsonResponseHandler::new(),
                self.config.error_handler.clone(),
                abort_signal,
                self.config.client.clone(),
            )
            .await?;

        let response = api_response.value;
        let response_headers = api_response.headers;

        let text = response
            .choices
            .first()
            .and_then(|c| c.text.clone())
            .unwrap_or_default();

        let finish_reason = map_openai_compatible_completion_finish_reason(
            response
                .choices
                .first()
                .and_then(|c| c.finish_reason.as_deref()),
        );
        let usage = convert_openai_compatible_completion_usage(response.usage.as_ref());

        let response_body = serde_json::to_value(&response).ok();
        let timestamp = response
            .created
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0));
        let response_id = response.id.clone();

        // Only include text content when non-empty
        let content = if text.is_empty() {
            Vec::new()
        } else {
            vec![AssistantContentPart::Text(TextPart {
                text,
                provider_metadata: None,
            })]
        };

        Ok(LanguageModelV4GenerateResult {
            content,
            usage,
            finish_reason,
            warnings,
            provider_metadata: None,
            request: Some(LanguageModelV4Request { body: Some(body) }),
            response: Some(LanguageModelV4Response {
                id: response_id,
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
        let mut warnings = Vec::new();
        let provider_name = self.config.provider_options_name();
        warn_if_deprecated_provider_options_key(
            provider_name,
            options.provider_options.as_ref(),
            &mut warnings,
        );
        let (compat_opts, passthrough) =
            extract_completion_options(&options.provider_options, provider_name);
        let prompt_result = convert_to_completion_prompt(&options.prompt)?;

        // Warn about unsupported features
        if options.top_k.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "topK".into(),
                details: Some("This model does not support topK. topK is ignored.".into()),
            });
        }
        if options.tools.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "tools".into(),
                details: Some("This model does not support tools.".into()),
            });
        }
        if options.tool_choice.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "toolChoice".into(),
                details: Some("This model does not support toolChoice.".into()),
            });
        }

        let mut body = json!({
            "model": self.model_id,
            "prompt": prompt_result.prompt,
            "stream": true,
        });

        if self.config.include_usage {
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

        // Merge stop sequences: user-provided + prompt-generated
        let mut all_stop = prompt_result.stop_sequences;
        if let Some(ref user_stop) = options.stop_sequences {
            all_stop.extend(user_stop.iter().cloned());
        }
        if !all_stop.is_empty() {
            body["stop"] = json!(all_stop);
        }

        if let Some(echo) = compat_opts.echo {
            body["echo"] = Value::Bool(echo);
        }
        if let Some(ref bias) = compat_opts.logit_bias {
            body["logit_bias"] = serde_json::to_value(bias).unwrap_or_default();
        }
        if let Some(ref suffix) = compat_opts.suffix {
            body["suffix"] = Value::String(suffix.clone());
        }
        if let Some(ref user) = compat_opts.user {
            body["user"] = Value::String(user.clone());
        }

        // Deep-merge extra_body onto the wire body. Callers own
        // wire-correct nesting; deep merge places nested overlays
        // at the right slot without clobbering sibling typed writes.
        if !passthrough.is_empty() {
            let overlay = Value::Object(passthrough.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // Apply request body transform
        let body = self.config.transform_body(body);

        let url = self.config.url("/completions");
        let headers = self.config.get_headers();

        let (byte_stream, response_headers) = post_stream_to_api_with_client_and_headers(
            &url,
            Some(headers),
            &body,
            abort_signal,
            self.config.client.clone(),
        )
        .await?;

        let request_body = body.clone();
        let include_raw = options.include_raw_chunks.unwrap_or(false);
        let stream = create_completion_stream(
            byte_stream,
            warnings,
            self.config.include_usage,
            include_raw,
        );

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

fn create_completion_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
    _include_usage: bool,
    include_raw: bool,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    use futures::StreamExt;

    struct State {
        byte_stream: vercel_ai_provider_utils::ByteStream,
        buffer: String,
        pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
        text_id: String,
        usage: Option<super::openai_compatible_completion_api::OpenAICompatibleCompletionUsage>,
        finish_reason: Option<String>,
        done: bool,
        finish_emitted: bool,
        metadata_emitted: bool,
        include_raw: bool,
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
            text_id: vercel_ai_provider_utils::generate_id("txt"),
            usage: None,
            finish_reason: None,
            done: false,
            finish_emitted: false,
            metadata_emitted: false,
            include_raw,
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

                                // Parse once as Value for reuse
                                let raw: serde_json::Value = match serde_json::from_str(data) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        state.finish_reason = Some("error".to_string());
                                        state.pending.push_back(LanguageModelV4StreamPart::Error {
                                            error: StreamError::new(format!(
                                                "Failed to parse completion chunk: {e}"
                                            )),
                                        });
                                        continue;
                                    }
                                };

                                // 1. Emit raw chunk BEFORE any validation (matches TS)
                                if state.include_raw {
                                    state.pending.push_back(LanguageModelV4StreamPart::Raw {
                                        raw_value: raw.clone(),
                                    });
                                }

                                // 2. Detect error chunks from the API
                                if let Some(error) = raw.get("error") {
                                    let message = error
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Unknown error");
                                    state.finish_reason = Some("error".to_string());
                                    state.pending.push_back(LanguageModelV4StreamPart::Error {
                                        error: StreamError::new(message),
                                    });
                                    continue;
                                }

                                match serde_json::from_value::<OpenAICompatibleCompletionChunk>(raw)
                                {
                                    Ok(chunk) => {
                                        // Emit ResponseMetadata + TextStart on first valid chunk
                                        if !state.metadata_emitted {
                                            state.metadata_emitted = true;
                                            let timestamp = chunk
                                                .created
                                                .and_then(|ts| {
                                                    chrono::DateTime::from_timestamp(ts as i64, 0)
                                                })
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
                                            state.pending.push_back(
                                                LanguageModelV4StreamPart::ResponseMetadata(meta),
                                            );

                                            // Emit TextStart unconditionally on first valid chunk
                                            state.pending.push_back(
                                                LanguageModelV4StreamPart::TextStart {
                                                    id: state.text_id.clone(),
                                                    provider_metadata: None,
                                                },
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
                                                if let Some(ref text) = choice.text
                                                    && !text.is_empty()
                                                {
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
                                    Err(e) => {
                                        state.finish_reason = Some("error".to_string());
                                        state.pending.push_back(LanguageModelV4StreamPart::Error {
                                            error: StreamError::new(format!(
                                                "Failed to parse completion chunk: {e}"
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
                        // Emit TextEnd if we saw any valid chunk
                        if state.metadata_emitted {
                            state.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                                id: state.text_id.clone(),
                                provider_metadata: None,
                            });
                        }
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            state.pending.push_back(LanguageModelV4StreamPart::Finish {
                                usage: convert_openai_compatible_completion_usage(
                                    state.usage.as_ref(),
                                ),
                                finish_reason: map_openai_compatible_completion_finish_reason(
                                    state.finish_reason.as_deref(),
                                ),
                                provider_metadata: None,
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

#[cfg(test)]
#[path = "openai_compatible_completion_language_model.test.rs"]
mod tests;
