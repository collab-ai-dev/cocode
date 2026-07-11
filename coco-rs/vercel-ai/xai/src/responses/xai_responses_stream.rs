use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::pin::Pin;

use futures::Stream;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::LanguageModelV4ToolResult;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::Source;
use vercel_ai_provider::StreamError;
use vercel_ai_provider::UnifiedFinishReason;
use vercel_ai_provider::Usage;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::ByteStream;
use vercel_ai_provider_utils::SseDecoder;
use vercel_ai_provider_utils::generate_id;

use super::convert_xai_responses_usage::convert_xai_responses_usage;
use super::map_xai_responses_finish_reason::map_xai_responses_finish_reason;
use super::xai_responses_api_types::ResponseAnnotation;
use super::xai_responses_api_types::ResponseMeta;
use super::xai_responses_api_types::ResponseOutputItem;
use super::xai_responses_api_types::ResponsesStreamEvent;
use super::xai_responses_api_types::ResponsesToolNames;
use super::xai_responses_api_types::resolve_server_tool;
use super::xai_responses_language_model::cost_provider_metadata;
use super::xai_responses_language_model::file_search_result;

/// Create a `LanguageModelV4StreamPart` stream from a raw xAI Responses SSE
/// byte stream. Mirrors the `TransformStream` in
/// `xai-responses-language-model.ts`.
pub fn create_xai_responses_stream(
    byte_stream: ByteStream,
    warnings: Vec<Warning>,
    include_raw: bool,
    tool_names: ResponsesToolNames,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    let stream = futures::stream::unfold(
        XaiResponsesStreamState::new(byte_stream, warnings, include_raw, tool_names),
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
                        state.flush();
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

struct XaiResponsesStreamState {
    byte_stream: ByteStream,
    decoder: SseDecoder,
    pending: VecDeque<LanguageModelV4StreamPart>,
    tool_names: ResponsesToolNames,
    include_raw: bool,
    finish_reason: FinishReason,
    has_function_call: bool,
    usage: Option<Usage>,
    cost_in_usd_ticks: Option<f64>,
    is_first_chunk: bool,
    /// Text block ids that have started (insertion-ordered for a deterministic
    /// `text-end` flush).
    text_blocks: Vec<String>,
    seen_tool_calls: HashSet<String>,
    /// Ongoing function calls keyed by `output_index`, so
    /// `function_call_arguments.delta` events can stream into them.
    ongoing_tool_calls: HashMap<u64, (String, String)>,
    active_reasoning: HashSet<String>,
    done: bool,
    finish_emitted: bool,
}

impl XaiResponsesStreamState {
    fn new(
        byte_stream: ByteStream,
        warnings: Vec<Warning>,
        include_raw: bool,
        tool_names: ResponsesToolNames,
    ) -> Self {
        let mut pending = VecDeque::new();
        pending.push_back(LanguageModelV4StreamPart::StreamStart { warnings });
        Self {
            byte_stream,
            decoder: SseDecoder::new(),
            pending,
            tool_names,
            include_raw,
            finish_reason: FinishReason {
                unified: UnifiedFinishReason::Other,
                raw: None,
            },
            has_function_call: false,
            usage: None,
            cost_in_usd_ticks: None,
            is_first_chunk: true,
            text_blocks: Vec::new(),
            seen_tool_calls: HashSet::new(),
            ongoing_tool_calls: HashMap::new(),
            active_reasoning: HashSet::new(),
            done: false,
            finish_emitted: false,
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

    /// Emit trailing `text-end` parts and the terminal `finish`.
    fn flush(&mut self) {
        if self.finish_emitted {
            return;
        }
        self.finish_emitted = true;
        let ids: Vec<String> = self.text_blocks.drain(..).collect();
        for id in ids {
            self.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                id,
                provider_metadata: None,
            });
        }
        let provider_metadata = self.cost_in_usd_ticks.map(cost_provider_metadata);
        self.pending.push_back(LanguageModelV4StreamPart::Finish {
            usage: self.usage.clone().unwrap_or_default(),
            finish_reason: self.finish_reason.clone(),
            provider_metadata,
        });
    }

    fn process_data_line(&mut self, data: &str) {
        let raw: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                self.emit_parse_error(format!("Failed to parse xAI chunk: {e}"));
                return;
            }
        };

        if self.include_raw {
            self.pending.push_back(LanguageModelV4StreamPart::Raw {
                raw_value: raw.clone(),
            });
        }

        let event: ResponsesStreamEvent = match serde_json::from_value(raw) {
            Ok(e) => e,
            Err(e) => {
                self.emit_parse_error(format!("Invalid xAI event structure: {e}"));
                return;
            }
        };

        self.handle_event(event);
    }

    /// Surface a malformed chunk as an `Error` part and mark the raw finish
    /// reason `"error"` (→ unified `Other`), mirroring the chat model.
    fn emit_parse_error(&mut self, message: String) {
        self.finish_reason = FinishReason {
            unified: UnifiedFinishReason::Other,
            raw: Some("error".to_string()),
        };
        self.pending.push_back(LanguageModelV4StreamPart::Error {
            error: StreamError::new(message),
        });
    }

    fn handle_event(&mut self, event: ResponsesStreamEvent) {
        match event {
            ResponsesStreamEvent::ResponseCreated { response }
            | ResponsesStreamEvent::ResponseInProgress { response } => {
                if self.is_first_chunk {
                    self.is_first_chunk = false;
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ResponseMetadata(
                            response_metadata(response.as_ref()),
                        ));
                }
            }

            ResponsesStreamEvent::ReasoningSummaryPartAdded { item_id } => {
                if !self.active_reasoning.contains(&item_id) {
                    self.active_reasoning.insert(item_id.clone());
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningStart {
                            id: reasoning_block_id(&item_id),
                            provider_metadata: Some(xai_item_meta(&item_id)),
                        });
                }
            }

            ResponsesStreamEvent::ReasoningSummaryTextDelta { item_id, delta } => {
                self.pending
                    .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                        id: reasoning_block_id(&item_id),
                        delta,
                        provider_metadata: Some(xai_item_meta(&item_id)),
                    });
            }

            ResponsesStreamEvent::ReasoningTextDelta { item_id, delta } => {
                if !self.active_reasoning.contains(&item_id) {
                    self.active_reasoning.insert(item_id.clone());
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningStart {
                            id: reasoning_block_id(&item_id),
                            provider_metadata: Some(xai_item_meta(&item_id)),
                        });
                }
                self.pending
                    .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                        id: reasoning_block_id(&item_id),
                        delta,
                        provider_metadata: Some(xai_item_meta(&item_id)),
                    });
            }

            ResponsesStreamEvent::ReasoningSummaryTextDone { .. }
            | ResponsesStreamEvent::ReasoningTextDone { .. }
            | ResponsesStreamEvent::FnCallArgsDone { .. }
            | ResponsesStreamEvent::CustomToolCallInputDelta { .. }
            | ResponsesStreamEvent::CustomToolCallInputDone { .. }
            | ResponsesStreamEvent::Unknown => {}

            ResponsesStreamEvent::OutputTextDelta { item_id, delta } => {
                let block_id = text_block_id(&item_id);
                if !self.text_blocks.iter().any(|b| b == &block_id) {
                    self.text_blocks.push(block_id.clone());
                    self.pending
                        .push_back(LanguageModelV4StreamPart::TextStart {
                            id: block_id.clone(),
                            provider_metadata: None,
                        });
                }
                self.pending
                    .push_back(LanguageModelV4StreamPart::TextDelta {
                        id: block_id,
                        delta,
                        provider_metadata: None,
                    });
            }

            ResponsesStreamEvent::OutputTextDone { annotations, .. } => {
                if let Some(annotations) = annotations {
                    for ann in &annotations {
                        self.push_source(ann);
                    }
                }
            }

            ResponsesStreamEvent::OutputTextAnnotationAdded { annotation, .. } => {
                self.push_source(&annotation);
            }

            ResponsesStreamEvent::ResponseCompleted { response }
            | ResponsesStreamEvent::ResponseDone { response } => {
                if let Some(resp) = response {
                    self.absorb_usage(&resp);
                    if let Some(status) = resp.status {
                        let unified = if self.has_function_call {
                            UnifiedFinishReason::ToolUse
                        } else {
                            map_xai_responses_finish_reason(Some(&status))
                        };
                        self.finish_reason = FinishReason {
                            unified,
                            raw: Some(status),
                        };
                    }
                }
            }

            ResponsesStreamEvent::ResponseIncomplete { response } => {
                if let Some(resp) = response {
                    self.absorb_usage(&resp);
                    let reason = resp.incomplete_details.and_then(|d| d.reason);
                    self.finish_reason = FinishReason {
                        unified: reason
                            .as_deref()
                            .map(|r| map_xai_responses_finish_reason(Some(r)))
                            .unwrap_or(UnifiedFinishReason::Other),
                        raw: Some(reason.unwrap_or_else(|| "incomplete".to_string())),
                    };
                }
            }

            ResponsesStreamEvent::ResponseFailed { response } => {
                if let Some(resp) = response {
                    self.absorb_usage(&resp);
                    // Surface any error message as an `Error` part (a coco
                    // addition matching the openai Rust model; the TS only sets
                    // finish state).
                    if let Some(message) = resp.error.as_ref().and_then(|e| e.message.clone()) {
                        self.pending.push_back(LanguageModelV4StreamPart::Error {
                            error: StreamError::new(message),
                        });
                    }
                    // A `response.failed` is a *server-declared* failure — the
                    // TS maps a missing/unmappable reason to unified `error`.
                    // (Distinct from a malformed chunk, which stays `Other`.)
                    let reason = resp.incomplete_details.and_then(|d| d.reason);
                    self.finish_reason = FinishReason {
                        unified: reason
                            .as_deref()
                            .map(|r| map_xai_responses_finish_reason(Some(r)))
                            .unwrap_or(UnifiedFinishReason::Error),
                        raw: Some(reason.unwrap_or_else(|| "error".to_string())),
                    };
                }
            }

            ResponsesStreamEvent::Error { message, .. } => {
                self.finish_reason = FinishReason {
                    unified: UnifiedFinishReason::Other,
                    raw: Some("error".to_string()),
                };
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: StreamError::new(message.unwrap_or_else(|| "Unknown error".to_string())),
                });
            }

            ResponsesStreamEvent::FnCallArgsDelta {
                output_index,
                delta,
                ..
            } => {
                if let Some((_, tool_call_id)) = self.ongoing_tool_calls.get(&output_index) {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id: tool_call_id.clone(),
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::OutputItemAdded { item, output_index } => {
                if let Some(item) = item {
                    self.handle_output_item(item, output_index, /*is_done*/ false);
                }
            }
            ResponsesStreamEvent::OutputItemDone { item, output_index } => {
                if let Some(item) = item {
                    self.handle_output_item(item, output_index, /*is_done*/ true);
                }
            }
        }
    }

    fn handle_output_item(&mut self, item: ResponseOutputItem, output_index: u64, is_done: bool) {
        // Reasoning: emit start (if missing) + end on the `done` boundary.
        if let ResponseOutputItem::Reasoning {
            id,
            encrypted_content,
            ..
        } = &item
        {
            if is_done {
                let item_id = id.clone().unwrap_or_default();
                let block_id = reasoning_block_id(&item_id);
                if !self.active_reasoning.contains(&item_id) {
                    self.active_reasoning.insert(item_id.clone());
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningStart {
                            id: block_id.clone(),
                            provider_metadata: Some(xai_item_meta(&item_id)),
                        });
                }
                self.pending
                    .push_back(LanguageModelV4StreamPart::ReasoningEnd {
                        id: block_id,
                        provider_metadata: Some(xai_reasoning_end_meta(
                            encrypted_content.as_deref(),
                            &item_id,
                        )),
                    });
                self.active_reasoning.remove(&item_id);
            }
            return;
        }

        if let ResponseOutputItem::FileSearchCall {
            id,
            queries,
            results,
            ..
        } = &item
        {
            let tool_name = self
                .tool_names
                .file_search
                .clone()
                .unwrap_or_else(|| "file_search".to_string());
            if !self.seen_tool_calls.contains(id) {
                self.seen_tool_calls.insert(id.clone());
                self.emit_server_tool_call(id, &tool_name, String::new());
            }
            if is_done {
                let result = file_search_result(queries.as_deref(), results.as_deref());
                self.pending
                    .push_back(LanguageModelV4StreamPart::ToolResult(
                        LanguageModelV4ToolResult::new(id.clone(), tool_name, result),
                    ));
            }
            return;
        }

        if let Some((kind, tool_item)) = item.as_server_tool() {
            let (tool_name, input) = resolve_server_tool(kind, tool_item, &self.tool_names);
            let id = tool_item.id.clone();
            let is_custom = matches!(
                kind,
                super::xai_responses_api_types::ServerToolKind::CustomToolCall
            );
            let should_emit = if is_custom { is_done } else { true };
            if should_emit && !self.seen_tool_calls.contains(&id) {
                self.seen_tool_calls.insert(id.clone());
                self.emit_server_tool_call(&id, &tool_name, input);
            }
            if is_done {
                self.pending
                    .push_back(LanguageModelV4StreamPart::ToolResult(
                        LanguageModelV4ToolResult::new(id, tool_name, json!({})),
                    ));
            }
            return;
        }

        match item {
            ResponseOutputItem::Message { id, content, .. } => {
                let item_id = id.unwrap_or_default();
                for cp in &content {
                    if let Some(text) = &cp.text
                        && !text.is_empty()
                    {
                        let block_id = text_block_id(&item_id);
                        if !self.text_blocks.iter().any(|b| b == &block_id) {
                            self.text_blocks.push(block_id.clone());
                            self.pending
                                .push_back(LanguageModelV4StreamPart::TextStart {
                                    id: block_id.clone(),
                                    provider_metadata: None,
                                });
                            self.pending
                                .push_back(LanguageModelV4StreamPart::TextDelta {
                                    id: block_id,
                                    delta: text.clone(),
                                    provider_metadata: None,
                                });
                        }
                    }
                    if let Some(annotations) = &cp.annotations {
                        for ann in annotations {
                            self.push_source(ann);
                        }
                    }
                }
            }
            ResponseOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                if is_done {
                    self.has_function_call = true;
                    self.ongoing_tool_calls.remove(&output_index);
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                            id: call_id.clone(),
                            provider_metadata: None,
                        });
                    self.pending.push_back(LanguageModelV4StreamPart::ToolCall(
                        LanguageModelV4ToolCall::new(call_id, name, arguments),
                    ));
                } else {
                    self.ongoing_tool_calls
                        .insert(output_index, (name.clone(), call_id.clone()));
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: call_id,
                            tool_name: name,
                            provider_executed: None,
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
            }
            _ => {}
        }
    }

    /// Emit the `tool-input-start → delta → end → tool-call` quartet for a
    /// provider-executed tool call.
    fn emit_server_tool_call(&mut self, id: &str, tool_name: &str, input: String) {
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolInputStart {
                id: id.to_string(),
                tool_name: tool_name.to_string(),
                provider_executed: None,
                dynamic: None,
                title: None,
                provider_metadata: None,
            });
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                id: id.to_string(),
                delta: input.clone(),
                provider_metadata: None,
            });
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                id: id.to_string(),
                provider_metadata: None,
            });
        self.pending.push_back(LanguageModelV4StreamPart::ToolCall(
            LanguageModelV4ToolCall::new(id.to_string(), tool_name.to_string(), input)
                .with_provider_executed(true),
        ));
    }

    fn absorb_usage(&mut self, resp: &ResponseMeta) {
        if let Some(usage) = &resp.usage {
            self.usage = Some(convert_xai_responses_usage(usage));
            self.cost_in_usd_ticks = usage.cost_in_usd_ticks;
        }
    }

    fn push_source(&mut self, annotation: &ResponseAnnotation) {
        if let Some((url, title)) = annotation.url_citation() {
            let mut src = Source::url(generate_id("src"), url.to_string());
            src.title = Some(title.unwrap_or(url).to_string());
            self.pending
                .push_back(LanguageModelV4StreamPart::Source(src));
        }
    }
}

fn text_block_id(item_id: &str) -> String {
    format!("text-{item_id}")
}

fn reasoning_block_id(item_id: &str) -> String {
    format!("reasoning-{item_id}")
}

/// `{ xai: { itemId } }`.
fn xai_item_meta(item_id: &str) -> ProviderMetadata {
    let mut pm = ProviderMetadata::new();
    pm.set("xai", json!({ "itemId": item_id }));
    pm
}

/// `{ xai: { reasoningEncryptedContent?, itemId? } }` for a reasoning end.
fn xai_reasoning_end_meta(encrypted: Option<&str>, item_id: &str) -> ProviderMetadata {
    let mut xai = serde_json::Map::new();
    if let Some(enc) = encrypted {
        xai.insert(
            "reasoningEncryptedContent".into(),
            Value::String(enc.to_string()),
        );
    }
    if !item_id.is_empty() {
        xai.insert("itemId".into(), Value::String(item_id.to_string()));
    }
    let mut pm = ProviderMetadata::new();
    pm.set("xai", Value::Object(xai));
    pm
}

/// Build the `response-metadata` part from a streaming lifecycle `response`.
fn response_metadata(response: Option<&ResponseMeta>) -> ResponseMetadata {
    let mut meta = ResponseMetadata::new();
    let Some(resp) = response else {
        return meta;
    };
    if let Some(id) = &resp.id {
        meta = meta.with_id(id.clone());
    }
    if let Some(model) = &resp.model {
        meta = meta.with_model(model.clone());
    }
    if let Some(ts) = resp
        .created_at
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|dt| dt.to_rfc3339())
    {
        meta = meta.with_timestamp(ts);
    }
    meta
}
