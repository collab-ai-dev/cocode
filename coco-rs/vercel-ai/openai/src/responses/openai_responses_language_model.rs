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
use vercel_ai_provider::content::ToolApprovalRequestPart;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::post_json_to_api_with_client_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_tapped;

use crate::openai_capabilities::SystemMessageMode;
use crate::openai_capabilities::get_capabilities;
use crate::openai_config::OpenAIConfig;
use crate::openai_config::ResponsesStorePolicy;
use crate::openai_error::OpenAIFailedResponseHandler;

use super::convert_responses_usage::convert_openai_responses_usage;
use super::convert_to_responses_input::ProviderToolFlags;
use super::convert_to_responses_input::convert_to_openai_responses_input_with_flags;
use super::map_finish_reason::map_openai_responses_finish_reason;
use super::openai_responses_api::OpenAIResponsesResponse;
use super::openai_responses_api::ResponseAnnotation;
use super::openai_responses_api::ResponseMessageContent;
use super::openai_responses_api::ResponseOutputItem;
use super::openai_responses_api::ResponsesStreamEvent;
use super::openai_responses_options::extract_responses_options;
use super::prepare_tools::prepare_responses_tools;
use super::provider_metadata::build_compaction_provider_metadata;
use super::provider_metadata::build_reasoning_provider_metadata;
use super::provider_metadata::build_responses_provider_metadata;
use super::provider_metadata::raw_reasoning_segment_id;
use super::provider_metadata::reasoning_text_marker;

/// OpenAI Responses API language model.
pub struct OpenAIResponsesLanguageModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAIResponsesLanguageModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    fn get_args(
        &self,
        options: &LanguageModelV4CallOptions,
    ) -> Result<(Value, Vec<Warning>), AISdkError> {
        let mut warnings = Vec::new();
        let (openai_options, raw_provider_options) =
            extract_responses_options(&options.provider_options);
        let layout = parse_prompt_layout_namespace(&options.provider_options);
        let caps = get_capabilities(&self.model_id);

        let force_reasoning = openai_options.force_reasoning.unwrap_or(false);
        let is_reasoning_model = force_reasoning || caps.is_reasoning_model;

        let system_message_mode =
            openai_options
                .system_message_mode
                .unwrap_or(if is_reasoning_model {
                    SystemMessageMode::Developer
                } else {
                    caps.system_message_mode
                });

        // Layout adapter wins for top-level instructions: when
        // `provider_options["prompt_layout"].instructions` is set, the
        // System messages must NOT be re-emitted into `input[]` —
        // otherwise the same text would appear in both top-level
        // `instructions` and the developer/system slot of `input[]`.
        let layout_instructions = layout.as_ref().and_then(|l| l.instructions.clone());
        let prompt_for_convert: std::borrow::Cow<'_, _> = if layout_instructions.is_some() {
            let filtered: Vec<_> = options
                .prompt
                .iter()
                .filter(|m| !matches!(m, vercel_ai_provider::LanguageModelV4Message::System { .. }))
                .cloned()
                .collect();
            std::borrow::Cow::Owned(filtered)
        } else {
            std::borrow::Cow::Borrowed(&options.prompt)
        };

        // Convert prompt to input items
        let tool_flags = ProviderToolFlags::from_tools(&options.tools);
        let (input, input_warnings) = convert_to_openai_responses_input_with_flags(
            &prompt_for_convert,
            system_message_mode,
            &tool_flags,
        );
        warnings.extend(input_warnings);

        // Prepare tools
        let prepared = prepare_responses_tools(&options.tools, &options.tool_choice);
        warnings.extend(prepared.warnings);

        // Unsupported parameter warnings for Responses API
        if options.top_k.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "topK".into(),
                details: Some("topK is not supported by the OpenAI Responses API".into()),
            });
        }
        if options.seed.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "seed".into(),
                details: Some("seed is not supported by the OpenAI Responses API".into()),
            });
        }
        if options.presence_penalty.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "presencePenalty".into(),
                details: Some(
                    "presencePenalty is not supported by the OpenAI Responses API".into(),
                ),
            });
        }
        if options.frequency_penalty.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "frequencyPenalty".into(),
                details: Some(
                    "frequencyPenalty is not supported by the OpenAI Responses API".into(),
                ),
            });
        }
        if options
            .stop_sequences
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        {
            warnings.push(Warning::Unsupported {
                feature: "stopSequences".into(),
                details: Some("stopSequences is not supported by the OpenAI Responses API".into()),
            });
        }

        // Warn about conversation + previousResponseId conflict
        if openai_options.conversation.is_some() && openai_options.previous_response_id.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "conversation + previousResponseId".into(),
                details: Some(
                    "conversation and previousResponseId should not be used together".into(),
                ),
            });
        }

        // Resolve reasoning effort: provider option takes precedence, then top-level reasoning.
        let reasoning_effort = openai_options.reasoning_effort.or_else(|| {
            if is_custom_reasoning(options.reasoning) {
                options.reasoning.and_then(|level| match level {
                    ReasoningLevel::Off => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::None)
                    }
                    ReasoningLevel::Minimal => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::Minimal)
                    }
                    ReasoningLevel::Low => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::Low)
                    }
                    ReasoningLevel::Medium => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::Medium)
                    }
                    ReasoningLevel::High => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::High)
                    }
                    ReasoningLevel::Xhigh => {
                        Some(crate::chat::openai_chat_options::ReasoningEffort::Xhigh)
                    }
                    ReasoningLevel::ProviderDefault => Option::None,
                })
            } else {
                Option::None
            }
        });
        let is_no_effort =
            reasoning_effort == Some(crate::chat::openai_chat_options::ReasoningEffort::None);
        let can_use_non_reasoning_params =
            is_no_effort && caps.supports_non_reasoning_params_with_no_effort;

        // Warn about reasoning model param conflicts
        if is_reasoning_model
            && (options.temperature.is_some() || options.top_p.is_some())
            && !can_use_non_reasoning_params
        {
            warnings.push(Warning::Unsupported {
                feature: "temperature/topP with reasoning model".into(),
                details: Some(
                    "temperature and topP are not supported with reasoning models unless \
                     reasoning_effort is 'none' and the model supports it"
                        .into(),
                ),
            });
        }

        // Warn about reasoning effort on non-reasoning models
        if !is_reasoning_model && reasoning_effort.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "reasoningEffort on non-reasoning model".into(),
                details: Some(
                    "reasoningEffort is only supported on reasoning models (o1, o3, o4-mini, gpt-5)"
                        .into(),
                ),
            });
        }

        let mut body = json!({
            "model": self.model_id,
            "input": input,
        });

        // Tools
        if let Some(tools) = prepared.tools {
            body["tools"] = Value::Array(tools);
        }
        if let Some(tc) = prepared.tool_choice {
            body["tool_choice"] = tc;
        }

        if is_reasoning_model {
            let mut reasoning = serde_json::Map::new();
            if let Some(effort) = reasoning_effort {
                reasoning.insert("effort".into(), Value::String(effort.as_str().into()));
            }
            if let Some(ref summary) = openai_options.reasoning_summary {
                reasoning.insert("summary".into(), Value::String(summary.clone()));
            }
            if !reasoning.is_empty() {
                body["reasoning"] = Value::Object(reasoning);
            }

            if let Some(max) = options.max_output_tokens {
                body["max_output_tokens"] = json!(max);
            }

            if can_use_non_reasoning_params {
                set_optional_f32(&mut body, "temperature", options.temperature);
                set_optional_f32(&mut body, "top_p", options.top_p);
            }
        } else {
            set_optional_f32(&mut body, "temperature", options.temperature);
            set_optional_f32(&mut body, "top_p", options.top_p);
            if let Some(max) = options.max_output_tokens {
                body["max_output_tokens"] = json!(max);
            }
        }

        // Response format: handled via `text` field
        if let Some(ref format) = options.response_format {
            match format {
                ResponseFormat::Text => {}
                ResponseFormat::Json {
                    schema,
                    name,
                    description,
                } => {
                    let strict = openai_options.strict_json_schema.unwrap_or(true);
                    if let Some(schema) = schema {
                        let schema_name = name.as_deref().unwrap_or("response");
                        let mut json_schema = json!({
                            "schema": schema,
                            "strict": strict,
                            "name": schema_name,
                        });
                        if let Some(desc) = description {
                            json_schema["description"] = Value::String(desc.clone());
                        }
                        body["text"] = json!({ "format": { "type": "json_schema", "json_schema": json_schema } });
                    } else {
                        body["text"] = json!({ "format": { "type": "json_object" } });
                    }
                }
            }
        }

        // Text verbosity
        if let Some(ref verbosity) = openai_options.text_verbosity {
            if body.get("text").is_none() {
                body["text"] = json!({});
            }
            body["text"]["verbosity"] = Value::String(verbosity.as_str().into());
        }

        // Provider options
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
        // `store` resolution for reasoning continuity, most-specific first:
        //   1. explicit per-call `store` always wins;
        //   2. ChatGPT subscription (codex backend) requires `store: false`;
        //   3. provider opt-in `reasoning_store = Stateless` forces
        //      `store: false` for reasoning models (codex-aligned, stateless;
        //      pairs with the `reasoning.encrypted_content` include below).
        // The default `ServerDefault` policy leaves plain API-key reasoning on
        // server-side state (store omitted) — NOT a hardcoded global false.
        let effective_store = openai_options
            .store
            .or_else(|| self.config.chatgpt_subscription.then_some(false))
            .or_else(|| {
                (is_reasoning_model
                    && self.config.reasoning_store == ResponsesStorePolicy::Stateless)
                    .then_some(false)
            });
        if let Some(store) = effective_store {
            body["store"] = Value::Bool(store);
        }
        if let Some(ref metadata) = openai_options.metadata {
            body["metadata"] = metadata.clone();
        }
        // Note: top-level `instructions` is written AFTER the
        // `merge_json_value` extras deep-merge below so the layout
        // slot wins over both the typed `openai_options.instructions`
        // and any raw `openai.*` map override.
        if let Some(ref conversation) = openai_options.conversation {
            body["conversation"] = Value::String(conversation.clone());
        }
        if let Some(ref prev_id) = openai_options.previous_response_id {
            body["previous_response_id"] = Value::String(prev_id.clone());
        }
        if let Some(max_tc) = openai_options.max_tool_calls {
            body["max_tool_calls"] = json!(max_tc);
        }
        if let Some(ref include) = openai_options.include {
            body["include"] = json!(include);
        }
        if let Some(ref truncation) = openai_options.truncation {
            body["truncation"] = Value::String(truncation.clone());
        }
        if let Some(ref tier) = openai_options.service_tier {
            body["service_tier"] = Value::String(tier.as_str().into());
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

        // Context management (server-side compaction)
        if let Some(ref cm) = openai_options.context_management {
            let cm_values: Vec<Value> = cm
                .iter()
                .map(|entry| {
                    json!({
                        "type": entry.entry_type,
                        "compact_threshold": entry.compact_threshold,
                    })
                })
                .collect();
            body["context_management"] = Value::Array(cm_values);
        }

        // Logprobs
        const TOP_LOGPROBS_MAX: u64 = 20;
        if let Some(ref logprobs) = openai_options.logprobs {
            match logprobs {
                Value::Bool(true) => {
                    body["top_logprobs"] = json!(TOP_LOGPROBS_MAX);
                    ensure_include_entry(&mut body, "message.output_text.logprobs");
                }
                Value::Number(n) => {
                    body["top_logprobs"] = Value::Number(n.clone());
                    ensure_include_entry(&mut body, "message.output_text.logprobs");
                }
                _ => {}
            }
        }

        // Auto-include: add sources and outputs for provider tools present in tools
        if let Some(ref tools) = options.tools {
            let has_web_search = tools.iter().any(|t| match t {
                vercel_ai_provider::LanguageModelV4Tool::Provider(pt) => {
                    pt.name == "web_search" || pt.name == "web_search_preview"
                }
                _ => false,
            });
            let has_code_interpreter = tools.iter().any(|t| match t {
                vercel_ai_provider::LanguageModelV4Tool::Provider(pt) => {
                    pt.name == "code_interpreter"
                }
                _ => false,
            });

            if has_web_search {
                ensure_include_entry(&mut body, "web_search_call.action.sources");
            }
            if has_code_interpreter {
                ensure_include_entry(&mut body, "code_interpreter_call.outputs");
            }
        }

        // Auto-include reasoning encrypted_content when store=false and reasoning model
        if is_reasoning_model && effective_store == Some(false) {
            ensure_include_entry(&mut body, "reasoning.encrypted_content");
        }

        // Deep-merge extra_body onto the wire body. Producers
        // (`coco_inference::thinking_convert`, user extras) own the
        // wire-correct nesting; deep merge places nested overlays at
        // the right slot without clobbering sibling typed writes.
        if !raw_provider_options.is_empty() {
            let overlay = Value::Object(raw_provider_options.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // Top-level `instructions` resolution. Layout slot wins over
        // both `openai_options.instructions` and the raw `openai.*` map
        // (which the deep-merge above just spliced in). Emit a
        // warning on conflict so callers notice the override.
        let instructions_to_write = match (
            layout_instructions.as_ref(),
            openai_options.instructions.as_ref(),
        ) {
            (Some(layout_instr), Some(_)) => {
                warnings.push(Warning::other(
                    "OpenAI `instructions` set both via prompt_layout and provider \
                     options; layout wins",
                ));
                Some(layout_instr.clone())
            }
            (Some(layout_instr), None) => Some(layout_instr.clone()),
            (None, Some(opt_instr)) => {
                // Already spliced via shallow_merge, but writing here
                // makes the typed-options path explicit and survives
                // a future change to the merge order.
                Some(opt_instr.clone())
            }
            (None, None) => None,
        };
        if let Some(instructions) = instructions_to_write {
            body["instructions"] = Value::String(instructions);
        }

        Ok((body, warnings))
    }
}

#[async_trait]
impl LanguageModelV4 for OpenAIResponsesLanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        let mut map = HashMap::new();
        if let Ok(re) = Regex::new(r"^https?://.*$") {
            map.insert("image/*".into(), vec![re.clone()]);
            map.insert("application/pdf".into(), vec![re]);
        }
        map
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let (body, warnings) = self.get_args(options)?;
        let url = self.config.url("/responses");
        let headers = self.config.get_headers();

        let response: OpenAIResponsesResponse = post_json_to_api_with_client_tapped(
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

        let mut content: Vec<AssistantContentPart> = Vec::new();
        let mut has_function_call = false;

        // Pre-collect hosted tool_search_call IDs for matching with tool_search_output
        let mut hosted_tool_search_call_ids: std::collections::VecDeque<String> = response
            .output
            .iter()
            .filter_map(|item| match item {
                ResponseOutputItem::ToolSearchCall { id, execution, .. }
                    if execution.as_deref() == Some("server") =>
                {
                    id.clone()
                }
                _ => None,
            })
            .collect();

        for item in &response.output {
            match item {
                ResponseOutputItem::Message {
                    content: msg_content,
                    ..
                } => {
                    for part in msg_content {
                        match part {
                            ResponseMessageContent::OutputText {
                                text,
                                annotations,
                                logprobs,
                            } => {
                                let text_meta = logprobs
                                    .as_ref()
                                    .filter(|lp| !lp.is_empty())
                                    .and_then(|lp| serde_json::to_value(lp).ok())
                                    .map(|v| {
                                        ProviderMetadata(HashMap::from([("logprobs".into(), v)]))
                                    });
                                if let Some(text) = text {
                                    content.push(AssistantContentPart::Text(TextPart {
                                        text: text.clone(),
                                        provider_metadata: text_meta,
                                    }));
                                }
                                if let Some(anns) = annotations {
                                    emit_annotations(anns, &mut content);
                                }
                            }
                            ResponseMessageContent::Refusal {
                                refusal: Some(text),
                            } => {
                                content.push(AssistantContentPart::Text(TextPart {
                                    text: text.clone(),
                                    provider_metadata: None,
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                ResponseOutputItem::FunctionCall {
                    id: item_id,
                    call_id,
                    name,
                    arguments,
                    namespace,
                } => {
                    has_function_call = true;
                    let tool_name = name.clone().unwrap_or_default();
                    let parsed_input = vercel_ai_provider_utils::parse_tool_arguments_or_empty(
                        arguments.as_deref().unwrap_or(""),
                        &tool_name,
                    );
                    // TS upstream #14789: surface the function_call's
                    // `namespace` (set when a server-executed
                    // tool_search dispatched to a deferred tool) under
                    // `provider_metadata.openai.namespace` alongside
                    // the existing `itemId`.
                    let mut openai_meta = serde_json::Map::new();
                    if let Some(id) = item_id.clone() {
                        openai_meta.insert("itemId".into(), Value::String(id));
                    }
                    if let Some(ns) = namespace.clone() {
                        openai_meta.insert("namespace".into(), Value::String(ns));
                    }
                    let provider_metadata = if openai_meta.is_empty() {
                        None
                    } else {
                        let mut meta = HashMap::new();
                        meta.insert("openai".to_string(), Value::Object(openai_meta));
                        Some(ProviderMetadata(meta))
                    };
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        // Correlate by the mandatory wire `call_id`; the item id
                        // rides `provider_metadata.openai.itemId`.
                        tool_call_id: call_id.clone(),
                        tool_name,
                        input: parsed_input,
                        provider_executed: None,
                        provider_metadata,
                        invalid: false,
                        invalid_reason: None,
                    }));
                }
                ResponseOutputItem::CustomToolCall {
                    id,
                    call_id,
                    name,
                    input,
                } => {
                    has_function_call = true;
                    let tool_name = name.clone().unwrap_or_default();
                    // Custom (freeform/grammar) tools deliver the model output as
                    // raw text, NOT JSON — keep it verbatim as a string. Running
                    // JSON-repair here would mangle a patch body containing
                    // `{ }`. The tool's `coerce_raw_string_input` wraps the raw
                    // string into the typed shape downstream. Custom tools are
                    // client-executed (`provider_executed: None`), matching the
                    // function-call path.
                    let raw_input = input.clone().unwrap_or_default();
                    // Correlate by wire `call_id` (item `id` is receive-only);
                    // stash the item id under `provider_metadata.openai.itemId`,
                    // mirroring the function_call path.
                    let provider_metadata = id.clone().map(|item_id| {
                        let mut openai_meta = serde_json::Map::new();
                        openai_meta.insert("itemId".into(), Value::String(item_id));
                        let mut m = HashMap::new();
                        m.insert("openai".to_string(), Value::Object(openai_meta));
                        ProviderMetadata(m)
                    });
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone(),
                        tool_name,
                        input: Value::String(raw_input),
                        provider_executed: None,
                        invalid: false,
                        invalid_reason: None,
                        provider_metadata,
                    }));
                }
                ResponseOutputItem::Reasoning {
                    summary,
                    content: reasoning_content,
                    encrypted_content,
                    ..
                } => {
                    // Emit ONE reasoning part per item carrying both the
                    // concatenated summary text AND the encrypted_content
                    // chain-of-thought blob under
                    // `provider_metadata.openai.encryptedContent`. Splitting
                    // these across separate parts (the old behavior) produced
                    // two reasoning items on sendback; one item keeps the
                    // chain intact for store=false continuity.
                    let summary_text = summary
                        .iter()
                        .flatten()
                        .filter_map(|s| s.text.as_deref())
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let meta = build_reasoning_provider_metadata(encrypted_content.as_ref());
                    // Skip a genuinely empty reasoning item (no summary, no
                    // chain blob); otherwise carry whichever is present.
                    if !summary_text.is_empty() || meta.is_some() {
                        content.push(AssistantContentPart::Reasoning(
                            vercel_ai_provider::ReasoningPart {
                                text: summary_text,
                                provider_metadata: meta,
                            },
                        ));
                    }
                    // Raw reasoning `content` channel — display-only, marked
                    // `reasoningType=text` so it is stripped on sendback.
                    let raw_text = reasoning_content
                        .iter()
                        .flatten()
                        .filter_map(|c| c.text.as_deref())
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if !raw_text.is_empty() {
                        content.push(AssistantContentPart::Reasoning(
                            vercel_ai_provider::ReasoningPart {
                                text: raw_text,
                                provider_metadata: Some(reasoning_text_marker()),
                            },
                        ));
                    }
                }
                // Provider-executed tools — emit as ToolCall with provider_executed flag
                ResponseOutputItem::WebSearchCall { id, .. } => {
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: id.clone().unwrap_or_default(),
                        tool_name: "web_search".into(),
                        input: json!({ "type": "web_search" }),
                        provider_executed: Some(true),
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    }));
                }
                ResponseOutputItem::FileSearchCall { id, results, .. } => {
                    let mut meta = None;
                    if let Some(r) = results
                        && let Ok(v) = serde_json::to_value(r)
                    {
                        meta = Some(ProviderMetadata(HashMap::from([("results".into(), v)])));
                    }
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: id.clone().unwrap_or_default(),
                        tool_name: "file_search".into(),
                        input: json!({ "type": "file_search" }),
                        provider_executed: Some(true),
                        provider_metadata: meta,
                        invalid: false,
                        invalid_reason: None,
                    }));
                }
                ResponseOutputItem::CodeInterpreterCall {
                    id, code, outputs, ..
                } => {
                    let call_id = id.clone().unwrap_or_default();
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone(),
                        tool_name: "code_interpreter".into(),
                        input: json!({ "type": "code_interpreter", "code": code }),
                        provider_executed: Some(true),
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    }));
                    // Emit tool result if outputs are present
                    if let Some(outs) = outputs {
                        content.push(AssistantContentPart::ToolResult(
                            vercel_ai_provider::ToolResultPart::new(
                                call_id,
                                "code_interpreter",
                                vercel_ai_provider::ToolResultContent::json(json!(outs)),
                            ),
                        ));
                    }
                }
                ResponseOutputItem::ImageGenerationCall { id, result, .. } => {
                    let call_id = id.clone().unwrap_or_default();
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone(),
                        tool_name: "image_generation".into(),
                        input: json!({ "type": "image_generation" }),
                        provider_executed: Some(true),
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    }));
                    if let Some(res) = result {
                        content.push(AssistantContentPart::ToolResult(
                            vercel_ai_provider::ToolResultPart::new(
                                call_id,
                                "image_generation",
                                vercel_ai_provider::ToolResultContent::json(res.clone()),
                            ),
                        ));
                    }
                }
                ResponseOutputItem::McpCall {
                    id,
                    name,
                    arguments,
                    server_label,
                    output,
                    error,
                } => {
                    let call_id = id.clone().unwrap_or_default();
                    let parsed_args: Value = arguments
                        .as_deref()
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or(Value::Null);
                    let mut meta_map = HashMap::new();
                    if let Some(label) = server_label {
                        meta_map.insert("serverLabel".into(), Value::String(label.clone()));
                    }
                    let meta = if meta_map.is_empty() {
                        None
                    } else {
                        Some(ProviderMetadata(meta_map))
                    };
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone(),
                        tool_name: name.clone().unwrap_or_default(),
                        input: parsed_args,
                        provider_executed: Some(true),
                        invalid: false,
                        invalid_reason: None,
                        provider_metadata: meta,
                    }));
                    // Emit result or error
                    let tool_name_str = name.clone().unwrap_or_default();
                    if let Some(err) = error {
                        content.push(AssistantContentPart::ToolResult(
                            vercel_ai_provider::ToolResultPart::new(
                                call_id,
                                &tool_name_str,
                                vercel_ai_provider::ToolResultContent::json(err.clone()),
                            )
                            .with_error(),
                        ));
                    } else if let Some(out) = output {
                        content.push(AssistantContentPart::ToolResult(
                            vercel_ai_provider::ToolResultPart::new(
                                call_id,
                                &tool_name_str,
                                vercel_ai_provider::ToolResultContent::json(out.clone()),
                            ),
                        ));
                    }
                }
                ResponseOutputItem::McpApprovalRequest { id, rest } => {
                    let approval_id = id.clone().unwrap_or_default();
                    let mut part = ToolApprovalRequestPart::new(approval_id.clone(), approval_id);
                    if let Some(name) = rest.get("name").and_then(|v| v.as_str()) {
                        part = part.with_tool_name(name);
                    }
                    if let Some(label) = rest.get("server_label").and_then(|v| v.as_str()) {
                        part = part.with_context(label);
                    }
                    content.push(AssistantContentPart::ToolApprovalRequest(part));
                }
                ResponseOutputItem::LocalShellCall {
                    id,
                    call_id,
                    action,
                    ..
                } => {
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone().or_else(|| id.clone()).unwrap_or_default(),
                        tool_name: "local_shell".into(),
                        input: action.clone().unwrap_or(Value::Null),
                        provider_executed: Some(true),
                        invalid: false,
                        invalid_reason: None,
                        provider_metadata: None,
                    }));
                }
                ResponseOutputItem::ShellCall {
                    id,
                    call_id,
                    action,
                    output,
                    ..
                } => {
                    let tc_id = call_id.clone().or_else(|| id.clone()).unwrap_or_default();
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: tc_id.clone(),
                        tool_name: "shell".into(),
                        input: action.clone().unwrap_or(Value::Null),
                        provider_executed: Some(true),
                        invalid: false,
                        invalid_reason: None,
                        provider_metadata: None,
                    }));
                    if let Some(outs) = output {
                        content.push(AssistantContentPart::ToolResult(
                            vercel_ai_provider::ToolResultPart::new(
                                tc_id,
                                "shell",
                                vercel_ai_provider::ToolResultContent::json(json!(outs)),
                            ),
                        ));
                    }
                }
                ResponseOutputItem::ApplyPatchCall {
                    id,
                    call_id,
                    operation,
                    ..
                } => {
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: call_id.clone().or_else(|| id.clone()).unwrap_or_default(),
                        tool_name: "apply_patch".into(),
                        input: operation.clone().unwrap_or(Value::Null),
                        provider_executed: Some(true),
                        invalid: false,
                        invalid_reason: None,
                        provider_metadata: None,
                    }));
                }
                ResponseOutputItem::ToolSearchCall {
                    id,
                    call_id,
                    execution,
                    arguments,
                    ..
                } => {
                    has_function_call = true;
                    let tc_id = call_id.clone().or_else(|| id.clone()).unwrap_or_default();
                    let is_hosted = execution.as_deref() == Some("server");
                    let input = json!({
                        "arguments": arguments,
                        "call_id": call_id,
                    });
                    let mut item_meta = HashMap::new();
                    if let Some(item_id) = id {
                        item_meta.insert("itemId".into(), Value::String(item_id.clone()));
                    }
                    let pm = if item_meta.is_empty() {
                        None
                    } else {
                        let mut pm = ProviderMetadata::default();
                        pm.0.insert(
                            "openai".into(),
                            Value::Object(item_meta.into_iter().collect()),
                        );
                        Some(pm)
                    };
                    content.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: tc_id,
                        tool_name: "tool_search".into(),
                        input,
                        provider_executed: if is_hosted { Some(true) } else { None },
                        provider_metadata: pm,
                        invalid: false,
                        invalid_reason: None,
                    }));
                }
                ResponseOutputItem::ToolSearchOutput {
                    id, call_id, tools, ..
                } => {
                    let tc_id = call_id
                        .clone()
                        .or_else(|| hosted_tool_search_call_ids.pop_front())
                        .or_else(|| id.clone())
                        .unwrap_or_default();
                    let result = json!({ "tools": tools });
                    let mut tr = vercel_ai_provider::ToolResultPart::new(
                        tc_id,
                        "tool_search",
                        vercel_ai_provider::ToolResultContent::json(result),
                    );
                    if let Some(item_id) = id {
                        let mut openai_map = serde_json::Map::new();
                        openai_map.insert("itemId".into(), Value::String(item_id.clone()));
                        let mut pm = ProviderMetadata::default();
                        pm.0.insert("openai".into(), Value::Object(openai_map));
                        tr = tr.with_metadata(pm);
                    }
                    content.push(AssistantContentPart::ToolResult(tr));
                }
                ResponseOutputItem::Compaction {
                    id,
                    encrypted_content,
                } => {
                    let pm = build_compaction_provider_metadata(
                        id.as_deref().unwrap_or_default(),
                        encrypted_content.as_deref(),
                    );
                    content.push(AssistantContentPart::Custom(
                        vercel_ai_provider::CustomPart::new("openai-compaction")
                            .with_provider_metadata(pm),
                    ));
                }
                _ => {}
            }
        }

        let finish_reason =
            map_openai_responses_finish_reason(response.status.as_deref(), has_function_call);
        let usage = convert_openai_responses_usage(response.usage.as_ref());
        let provider_metadata = build_responses_provider_metadata(
            response.id.as_deref(),
            response.service_tier.as_deref(),
        );

        Ok(LanguageModelV4GenerateResult {
            content,
            usage,
            finish_reason,
            warnings,
            provider_metadata,
            request: Some(LanguageModelV4Request { body: Some(body) }),
            response: Some(LanguageModelV4Response {
                id: response.id.clone(),
                // Responses API delivers `created_at` as an ISO 8601
                // string (or unix-seconds number, normalized to string
                // by `deserialize_created_at`); parse to typed
                // DateTime here so the wire format is consistent with
                // the other providers.
                timestamp: response
                    .created_at
                    .as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
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
        body["stream"] = Value::Bool(true);

        let include_raw = options.include_raw_chunks.unwrap_or(false);
        let url = self.config.url("/responses");
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
        let stream = create_responses_stream(byte_stream, warnings, include_raw);

        Ok(LanguageModelV4StreamResult {
            stream,
            request: Some(LanguageModelV4Request {
                body: Some(request_body),
            }),
            response: Some(LanguageModelV4StreamResponse::new()),
        })
    }
}

// --- Streaming state machine ---

struct ActiveTextItem {
    started: bool,
}

struct ActiveFnCall {
    /// Wire item id (`fc_…`) — receive-only; rides
    /// `provider_metadata.openai.itemId`.
    id: String,
    /// Wire `call_id` (`call_…`) — the correlation id echoed back as the
    /// `function_call_output.call_id`. Seeded from `output_item.added`'s
    /// mandatory `call_id`; the single correlation id for every emit.
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
    /// Namespace marker from server-executed tool_search dispatch
    /// (TS upstream #14789). Surfaces under
    /// `provider_metadata.openai.namespace` on the finalized
    /// `tool-input-end` and `tool-call` events.
    namespace: Option<String>,
}

struct ActiveCustomToolCall {
    /// Wire item id (`ctc_…`) — receive-only; rides
    /// `provider_metadata.openai.itemId`.
    id: String,
    /// Wire `call_id` — echoed back as the `custom_tool_call_output.call_id`.
    /// Seeded from `output_item.added`'s mandatory `call_id`.
    call_id: String,
    name: String,
    input: String,
    started: bool,
}

struct ActiveReasoning {
    started: bool,
}

fn create_responses_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    warnings: Vec<Warning>,
    include_raw: bool,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    let stream = futures::stream::unfold(
        ResponsesStreamState::new(byte_stream, warnings, include_raw),
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
                        if !state.finish_emitted {
                            state.finish_emitted = true;
                            let finish = LanguageModelV4StreamPart::Finish {
                                usage: convert_openai_responses_usage(state.usage.as_ref()),
                                finish_reason: map_openai_responses_finish_reason(
                                    state.status.as_deref(),
                                    state.has_function_call,
                                ),
                                provider_metadata: build_responses_provider_metadata(
                                    state.response_id.as_deref(),
                                    state.service_tier.as_deref(),
                                ),
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

struct ResponsesStreamState {
    byte_stream: vercel_ai_provider_utils::ByteStream,
    buffer: String,
    pending: std::collections::VecDeque<LanguageModelV4StreamPart>,
    active_texts: HashMap<String, ActiveTextItem>,
    active_fn_calls: HashMap<String, ActiveFnCall>,
    active_custom_calls: HashMap<String, ActiveCustomToolCall>,
    /// Summary reasoning channel (`reasoning_summary_text.*`), keyed by item id.
    active_reasoning: HashMap<String, ActiveReasoning>,
    /// Raw reasoning channel (`reasoning_text.*`), tracked separately so it
    /// never collides with the summary channel of the same item. Surfaces as
    /// a distinct segment marked `provider_metadata.openai.reasoningType=text`.
    active_reasoning_content: HashMap<String, ActiveReasoning>,
    usage: Option<super::convert_responses_usage::OpenAIResponsesUsage>,
    status: Option<String>,
    has_function_call: bool,
    hosted_tool_search_call_ids: std::collections::VecDeque<String>,
    finish_emitted: bool,
    done: bool,
    metadata_emitted: bool,
    include_raw: bool,
    response_id: Option<String>,
    service_tier: Option<String>,
}

impl ResponsesStreamState {
    fn new(
        byte_stream: vercel_ai_provider_utils::ByteStream,
        warnings: Vec<Warning>,
        include_raw: bool,
    ) -> Self {
        let mut pending = std::collections::VecDeque::new();
        pending.push_back(LanguageModelV4StreamPart::StreamStart { warnings });

        Self {
            byte_stream,
            buffer: String::new(),
            pending,
            active_texts: HashMap::new(),
            active_fn_calls: HashMap::new(),
            active_custom_calls: HashMap::new(),
            active_reasoning: HashMap::new(),
            active_reasoning_content: HashMap::new(),
            usage: None,
            status: None,
            has_function_call: false,
            hosted_tool_search_call_ids: std::collections::VecDeque::new(),
            finish_emitted: false,
            done: false,
            metadata_emitted: false,
            include_raw,
            response_id: None,
            service_tier: None,
        }
    }

    /// Emit the closing `ToolInputEnd` + `ToolCall` for a function call.
    ///
    /// The correlation id is the wire `call_id` (NOT the item id), so the
    /// eventual `function_call_output.call_id` matches what the model
    /// expects; the item id and any tool_search `namespace` ride
    /// `provider_metadata.openai`. Emits a `ToolInputStart` first when no
    /// argument delta ever opened the call (zero-arg / `output_item.done`-only
    /// calls) so the start/end/call ids stay paired in the accumulator.
    fn finalize_fn_call(&mut self, fc: ActiveFnCall) {
        let emit_id = fc.call_id;
        let mut openai_meta = serde_json::Map::new();
        openai_meta.insert("itemId".into(), Value::String(fc.id.clone()));
        if let Some(ns) = &fc.namespace {
            openai_meta.insert("namespace".into(), Value::String(ns.clone()));
        }
        let mut meta_map = HashMap::new();
        meta_map.insert("openai".to_string(), Value::Object(openai_meta));
        let meta = ProviderMetadata(meta_map);
        if !fc.started {
            self.pending
                .push_back(LanguageModelV4StreamPart::ToolInputStart {
                    id: emit_id.clone(),
                    tool_name: fc.name.clone(),
                    provider_executed: None,
                    dynamic: None,
                    title: None,
                    provider_metadata: None,
                });
        }
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                id: emit_id.clone(),
                provider_metadata: Some(meta.clone()),
            });
        let tc = vercel_ai_provider::LanguageModelV4ToolCall::new(emit_id, fc.name, fc.arguments)
            .with_metadata(meta);
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
    }

    /// Closing emit for a custom (freeform/grammar) tool call. Same
    /// `call_id`-over-item-id correlation rule as [`Self::finalize_fn_call`];
    /// the item id rides `provider_metadata.openai.itemId`.
    fn finalize_custom_call(&mut self, ct: ActiveCustomToolCall) {
        let emit_id = ct.call_id;
        let mut openai_meta = serde_json::Map::new();
        openai_meta.insert("itemId".into(), Value::String(ct.id.clone()));
        let mut meta_map = HashMap::new();
        meta_map.insert("openai".to_string(), Value::Object(openai_meta));
        let meta = ProviderMetadata(meta_map);
        if !ct.started {
            self.pending
                .push_back(LanguageModelV4StreamPart::ToolInputStart {
                    id: emit_id.clone(),
                    tool_name: ct.name.clone(),
                    provider_executed: None,
                    dynamic: None,
                    title: None,
                    provider_metadata: None,
                });
        }
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                id: emit_id.clone(),
                provider_metadata: Some(meta.clone()),
            });
        let tc = vercel_ai_provider::LanguageModelV4ToolCall::new(emit_id, ct.name, ct.input)
            .with_metadata(meta);
        self.pending
            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
    }

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
                self.process_event(&data);
                continue;
            }
            self.buffer.drain(..line_len);
        }
    }

    fn process_event(&mut self, data: &str) {
        // Emit raw
        if self.include_raw
            && let Ok(raw) = serde_json::from_str::<Value>(data)
        {
            self.pending
                .push_back(LanguageModelV4StreamPart::Raw { raw_value: raw });
        }

        let event: ResponsesStreamEvent = match serde_json::from_str(data) {
            Ok(e) => e,
            Err(err) => {
                // Don't silently drop an SSE payload we can't model — an
                // unmodeled shape is often exactly what's needed when a turn
                // fails with no typed signal.
                tracing::warn!(
                    error = %err,
                    raw = %data,
                    "responses stream: undecodable SSE data event",
                );
                return;
            }
        };

        match event {
            ResponsesStreamEvent::ResponseCreated {
                response: Some(resp),
            } => {
                if !self.metadata_emitted {
                    self.metadata_emitted = true;
                    let mut meta = ResponseMetadata::new();
                    if let Some(ref id) = resp.id {
                        meta = meta.with_id(id.clone());
                    }
                    if let Some(ref model) = resp.model {
                        meta = meta.with_model(model.clone());
                    }
                    if let Some(ref tier) = resp.service_tier {
                        let mut openai_obj = serde_json::Map::new();
                        openai_obj.insert("serviceTier".into(), Value::String(tier.clone()));
                        let mut pm = ProviderMetadata::default();
                        pm.0.insert("openai".into(), Value::Object(openai_obj));
                        meta = meta.with_provider_metadata(pm);
                    }
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ResponseMetadata(meta));
                }
                // Track response_id and service_tier for Finish metadata
                self.response_id = resp.id;
                self.service_tier = resp.service_tier;
            }

            ResponsesStreamEvent::ResponseCompleted {
                response: Some(resp),
            } => {
                self.usage = resp.usage;
                self.status = resp.status;
            }

            ResponsesStreamEvent::ResponseIncomplete {
                response: Some(resp),
            } => {
                self.usage = resp.usage;
                // `incomplete` carries its real reason in
                // `incomplete_details.reason` (e.g. `max_output_tokens`,
                // `content_filter`) — the literal `status` is just
                // `"incomplete"`. Surface the reason so the finish maps to
                // `MaxTokens`/`ContentFilter` and the engine's output-budget
                // escalation can fire. Mirrors codex's `response.incomplete`
                // handling.
                let reason = resp
                    .incomplete_details
                    .as_ref()
                    .and_then(|d| d.get("reason"))
                    .and_then(|r| r.as_str())
                    .map(String::from);
                self.status = reason.or(resp.status);
            }

            ResponsesStreamEvent::ResponseFailed {
                response: Some(resp),
            } => {
                self.usage = resp.usage;
                // A mid-stream `response.failed` carries its classification in
                // `error.code` (NOT `incomplete_details`). Mirror codex's
                // classifier (codex-api/src/sse/responses.rs:325-359): a
                // context-window overflow routes through the typed
                // `ContextWindowExceeded` finish reason so reactive compaction
                // fires; every other failure surfaces as an `Error` stream
                // part (retryable for transient/overload codes, fatal for
                // quota/policy) instead of collapsing to a silent clean finish.
                let error_code = resp
                    .error
                    .as_ref()
                    .and_then(|e| e.get("code"))
                    .and_then(|c| c.as_str())
                    .map(String::from);
                let error_message = resp
                    .error
                    .as_ref()
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from);
                if let Some(err) = &resp.error {
                    tracing::warn!(
                        error = %err,
                        code = error_code.as_deref().unwrap_or(""),
                        "responses stream: response.failed",
                    );
                }
                match error_code.as_deref() {
                    Some("context_length_exceeded") => {
                        // Do NOT emit an Error — let the synthesized Finish
                        // carry `ContextWindowExceeded` so `app/query` runs
                        // reactive compaction instead of failing the turn.
                        self.status = Some("context_length_exceeded".into());
                    }
                    code => {
                        // codex treats quota/usage/invalid-prompt/cyber-policy
                        // as fatal and everything else (overload, slow_down,
                        // unclassified) as retryable. Preserve that split on
                        // the `is_retryable` flag.
                        let is_retryable = !matches!(
                            code,
                            Some("insufficient_quota")
                                | Some("usage_not_included")
                                | Some("invalid_prompt")
                                | Some("cyber_policy")
                        );
                        let message = error_message
                            .or_else(|| {
                                error_code
                                    .clone()
                                    .map(|c| format!("OpenAI responses failed (code: {c})"))
                            })
                            .unwrap_or_else(|| "OpenAI responses failed".into());
                        self.pending.push_back(LanguageModelV4StreamPart::Error {
                            error: vercel_ai_provider::StreamError {
                                message,
                                code: error_code,
                                is_retryable,
                            },
                        });
                        self.status = Some("error".into());
                    }
                }
            }

            ResponsesStreamEvent::OutputItemAdded { item: Some(item) } => match &item {
                ResponseOutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    namespace,
                    ..
                } => {
                    self.has_function_call = true;
                    let item_id = id.clone().unwrap_or_default();
                    self.active_fn_calls.insert(
                        item_id.clone(),
                        ActiveFnCall {
                            id: item_id,
                            // Seed the mandatory wire `call_id` now — the
                            // argument deltas carry only `item_id`, so this is
                            // the only place to capture the correlation id.
                            call_id: call_id.clone(),
                            name: name.clone().unwrap_or_default(),
                            arguments: String::new(),
                            started: false,
                            namespace: namespace.clone(),
                        },
                    );
                }
                ResponseOutputItem::CustomToolCall {
                    id, call_id, name, ..
                } => {
                    self.has_function_call = true;
                    let item_id = id.clone().unwrap_or_default();
                    self.active_custom_calls.insert(
                        item_id.clone(),
                        ActiveCustomToolCall {
                            id: item_id,
                            call_id: call_id.clone(),
                            name: name.clone().unwrap_or_default(),
                            input: String::new(),
                            started: false,
                        },
                    );
                }
                ResponseOutputItem::Message { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.active_texts
                        .insert(item_id, ActiveTextItem { started: false });
                }
                ResponseOutputItem::Reasoning { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.active_reasoning
                        .insert(item_id, ActiveReasoning { started: false });
                }
                // Provider-executed tool starts — emit ToolInputStart
                ResponseOutputItem::WebSearchCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "web_search".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::FileSearchCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "file_search".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::CodeInterpreterCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "code_interpreter".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::ImageGenerationCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "image_generation".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::ShellCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "shell".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::LocalShellCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "local_shell".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::ApplyPatchCall { id, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: "apply_patch".into(),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::McpCall { id, name, .. } => {
                    let item_id = id.clone().unwrap_or_default();
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputStart {
                            id: item_id,
                            tool_name: name.clone().unwrap_or_else(|| "mcp".into()),
                            provider_executed: Some(true),
                            dynamic: None,
                            title: None,
                            provider_metadata: None,
                        });
                }
                ResponseOutputItem::ToolSearchCall { id, execution, .. } => {
                    self.has_function_call = true;
                    let item_id = id.clone().unwrap_or_default();
                    let is_hosted = execution.as_deref() == Some("server");
                    if is_hosted {
                        self.hosted_tool_search_call_ids.push_back(item_id);
                    } else {
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputStart {
                                id: item_id,
                                tool_name: "tool_search".into(),
                                provider_executed: None,
                                dynamic: None,
                                title: None,
                                provider_metadata: None,
                            });
                    }
                }
                _ => {}
            },

            ResponsesStreamEvent::OutputTextDelta { item_id, delta, .. } => {
                if let (Some(item_id), Some(delta)) = (item_id, delta) {
                    // Emit TextStart if not yet
                    if let Some(text_item) = self.active_texts.get_mut(&item_id) {
                        if !text_item.started {
                            text_item.started = true;
                            self.pending
                                .push_back(LanguageModelV4StreamPart::TextStart {
                                    id: item_id.clone(),
                                    provider_metadata: None,
                                });
                        }
                    } else {
                        // Auto-create if not tracked
                        self.active_texts
                            .insert(item_id.clone(), ActiveTextItem { started: true });
                        self.pending
                            .push_back(LanguageModelV4StreamPart::TextStart {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                    }
                    self.pending
                        .push_back(LanguageModelV4StreamPart::TextDelta {
                            id: item_id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::OutputTextDone {
                item_id: Some(item_id),
                ..
            } => {
                self.pending.push_back(LanguageModelV4StreamPart::TextEnd {
                    id: item_id.clone(),
                    provider_metadata: None,
                });
                self.active_texts.remove(&item_id);
            }

            ResponsesStreamEvent::FnCallArgsDelta { item_id, delta } => {
                if let (Some(item_id), Some(delta)) = (item_id, delta)
                    && let Some(fc) = self.active_fn_calls.get_mut(&item_id)
                {
                    // Emit under the wire `call_id` so the start/delta/end/call
                    // ids all pair to one accumulator segment.
                    let emit_id = fc.call_id.clone();
                    if !fc.started {
                        fc.started = true;
                        let tool_name = fc.name.clone();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputStart {
                                id: emit_id.clone(),
                                tool_name,
                                provider_executed: None,
                                dynamic: None,
                                title: None,
                                provider_metadata: None,
                            });
                    }
                    fc.arguments.push_str(&delta);
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id: emit_id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::FnCallArgsDone {
                item_id: Some(item_id),
                ..
            } => {
                if let Some(fc) = self.active_fn_calls.remove(&item_id) {
                    self.finalize_fn_call(fc);
                }
            }

            ResponsesStreamEvent::OutputTextAnnotationAdded {
                annotation: Some(ResponseAnnotation::UrlCitation { url, title, .. }),
                ..
            } => {
                self.pending.push_back(LanguageModelV4StreamPart::Source(
                    vercel_ai_provider::content::SourcePart {
                        source_type: SourceType::Url,
                        id: vercel_ai_provider_utils::generate_id("src"),
                        url,
                        title,
                        media_type: None,
                        filename: None,
                        provider_metadata: None,
                    },
                ));
            }

            // Custom tool call streaming
            ResponsesStreamEvent::CustomToolCallInputDelta { item_id, delta } => {
                if let (Some(item_id), Some(delta)) = (item_id, delta)
                    && let Some(ct) = self.active_custom_calls.get_mut(&item_id)
                {
                    let emit_id = ct.call_id.clone();
                    if !ct.started {
                        ct.started = true;
                        let tool_name = ct.name.clone();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputStart {
                                id: emit_id.clone(),
                                tool_name,
                                // Custom (freeform/grammar) tools are client-
                                // executed — coco runs them locally (apply_patch).
                                provider_executed: None,
                                dynamic: None,
                                title: None,
                                provider_metadata: None,
                            });
                    }
                    ct.input.push_str(&delta);
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id: emit_id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::CustomToolCallInputDone {
                item_id: Some(item_id),
                ..
            } => {
                if let Some(ct) = self.active_custom_calls.remove(&item_id) {
                    self.finalize_custom_call(ct);
                }
            }

            // Reasoning lifecycle
            ResponsesStreamEvent::ReasoningSummaryDelta { item_id, delta } => {
                if let (Some(id), Some(delta)) = (item_id, delta) {
                    // Emit ReasoningStart if this is the first delta for this item
                    if let Some(r) = self.active_reasoning.get_mut(&id) {
                        if !r.started {
                            r.started = true;
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ReasoningStart {
                                    id: id.clone(),
                                    provider_metadata: None,
                                });
                        }
                    } else {
                        self.active_reasoning
                            .insert(id.clone(), ActiveReasoning { started: true });
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ReasoningStart {
                                id: id.clone(),
                                provider_metadata: None,
                            });
                    }
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                            id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::ReasoningSummaryDone { .. } => {
                // The reasoning segment is closed by `output_item.done` (which
                // carries `encrypted_content`), NOT here. Emitting
                // `ReasoningEnd` now would close the accumulator segment
                // before the chain-of-thought blob arrives, leaving nowhere to
                // attach it. Keep the `active_reasoning` entry open so the
                // `OutputItemDone` reasoning arm can finalize it.
            }

            ResponsesStreamEvent::ReasoningSummaryPartAdded {
                item_id: Some(id),
                summary_index,
                ..
            } => {
                // A new summary section (`reasoning.summary='detailed'`). For
                // sections after the first, emit a blank-line break so the
                // parts don't run together. A `"\n\n"` delta avoids the
                // End/Start churn the accumulator would ignore for an
                // already-active id.
                if summary_index.unwrap_or(0) > 0
                    && self.active_reasoning.get(&id).is_some_and(|r| r.started)
                {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                            id,
                            delta: "\n\n".into(),
                            provider_metadata: None,
                        });
                }
            }

            // Raw reasoning channel (`reasoning_text.*`) — distinct from the
            // condensed summary. Tracked in its own map and marked
            // `reasoningType=text` so it renders live but is stripped on
            // sendback (the server rehydrates it from `encrypted_content`).
            ResponsesStreamEvent::ReasoningTextDelta { item_id, delta, .. } => {
                if let (Some(id), Some(delta)) = (item_id, delta) {
                    let emit_id = raw_reasoning_segment_id(&id);
                    let entry = self
                        .active_reasoning_content
                        .entry(id)
                        .or_insert(ActiveReasoning { started: false });
                    if !entry.started {
                        entry.started = true;
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ReasoningStart {
                                id: emit_id.clone(),
                                provider_metadata: Some(reasoning_text_marker()),
                            });
                    }
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningDelta {
                            id: emit_id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::ReasoningTextDone {
                item_id: Some(id), ..
            } => {
                if self.active_reasoning_content.remove(&id).is_some() {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ReasoningEnd {
                            id: raw_reasoning_segment_id(&id),
                            provider_metadata: Some(reasoning_text_marker()),
                        });
                }
            }

            // Code interpreter streaming
            ResponsesStreamEvent::CodeInterpreterCodeDelta { item_id, delta } => {
                if let (Some(id), Some(delta)) = (item_id, delta) {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::CodeInterpreterCodeDone { .. } => {
                // Code completion handled by OutputItemDone
            }

            // Apply patch streaming
            ResponsesStreamEvent::ApplyPatchDiffDelta { item_id, delta } => {
                if let (Some(id), Some(delta)) = (item_id, delta) {
                    self.pending
                        .push_back(LanguageModelV4StreamPart::ToolInputDelta {
                            id,
                            delta,
                            provider_metadata: None,
                        });
                }
            }

            ResponsesStreamEvent::ApplyPatchDiffDone { .. } => {
                // Completion handled by OutputItemDone
            }

            // OutputItemDone — emit ToolCall + ToolResult for provider-executed tools
            ResponsesStreamEvent::OutputItemDone { item: Some(item) } => {
                match &item {
                    ResponseOutputItem::WebSearchCall { id, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        self.pending.push_back(LanguageModelV4StreamPart::ToolCall(
                            vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                                &item_id,
                                "web_search",
                                json!({ "type": "web_search" }),
                            )
                            .with_provider_executed(true),
                        ));
                    }
                    ResponseOutputItem::FileSearchCall { id, results, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "file_search",
                            json!({ "type": "file_search" }),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                        // Emit results as ToolResult
                        if let Some(r) = results {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::new(
                                        &item_id,
                                        "file_search",
                                        json!(r),
                                    ),
                                ));
                        }
                    }
                    ResponseOutputItem::CodeInterpreterCall {
                        id, code, outputs, ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "code_interpreter",
                            json!({ "type": "code_interpreter", "code": code }),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                        if let Some(outs) = outputs {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::new(
                                        &item_id,
                                        "code_interpreter",
                                        json!(outs),
                                    ),
                                ));
                        }
                    }
                    ResponseOutputItem::ImageGenerationCall { id, result, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "image_generation",
                            json!({ "type": "image_generation" }),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                        if let Some(res) = result {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::new(
                                        &item_id,
                                        "image_generation",
                                        res.clone(),
                                    ),
                                ));
                        }
                    }
                    ResponseOutputItem::ShellCall {
                        id, action, output, ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "shell",
                            action.clone().unwrap_or(Value::Null),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                        if let Some(outs) = output {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::new(
                                        &item_id,
                                        "shell",
                                        json!(outs),
                                    ),
                                ));
                        }
                    }
                    ResponseOutputItem::LocalShellCall { id, action, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "local_shell",
                            action.clone().unwrap_or(Value::Null),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                    }
                    ResponseOutputItem::ApplyPatchCall { id, operation, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            "apply_patch",
                            operation.clone().unwrap_or(Value::Null),
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                    }
                    ResponseOutputItem::McpCall {
                        id,
                        name,
                        arguments,
                        output,
                        error,
                        ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        let tool_name = name.clone().unwrap_or_else(|| "mcp".into());
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id.clone(),
                                provider_metadata: None,
                            });
                        let parsed_args: Value = arguments
                            .as_deref()
                            .and_then(|a| serde_json::from_str(a).ok())
                            .unwrap_or(Value::Null);
                        let tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &item_id,
                            &tool_name,
                            parsed_args,
                        )
                        .with_provider_executed(true);
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                        if let Some(err) = error {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::error(
                                        &item_id,
                                        &tool_name,
                                        err.clone(),
                                    ),
                                ));
                        } else if let Some(out) = output {
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ToolResult(
                                    vercel_ai_provider::LanguageModelV4ToolResult::new(
                                        &item_id,
                                        &tool_name,
                                        out.clone(),
                                    ),
                                ));
                        }
                    }
                    ResponseOutputItem::McpApprovalRequest { id, rest } => {
                        let approval_id = id.clone().unwrap_or_default();
                        let req = vercel_ai_provider::language_model::v4::LanguageModelV4ToolApprovalRequest::new(
                            approval_id.clone(),
                            approval_id,
                        );
                        let _ = rest;
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolApprovalRequest(req));
                    }
                    ResponseOutputItem::ToolSearchCall {
                        id,
                        call_id,
                        execution,
                        arguments,
                        ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        let tc_id = call_id.clone().unwrap_or_else(|| item_id.clone());
                        let is_hosted = execution.as_deref() == Some("server");
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolInputEnd {
                                id: item_id,
                                provider_metadata: None,
                            });
                        let input = json!({
                            "arguments": arguments,
                            "call_id": call_id,
                        });
                        let mut tc = vercel_ai_provider::LanguageModelV4ToolCall::from_json(
                            &tc_id,
                            "tool_search",
                            input,
                        );
                        if is_hosted {
                            tc = tc.with_provider_executed(true);
                        }
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolCall(tc));
                    }
                    ResponseOutputItem::ToolSearchOutput {
                        id, call_id, tools, ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        let tc_id = call_id
                            .clone()
                            .or_else(|| self.hosted_tool_search_call_ids.pop_front())
                            .unwrap_or_else(|| item_id.clone());
                        let result = json!({ "tools": tools });
                        self.pending
                            .push_back(LanguageModelV4StreamPart::ToolResult(
                                vercel_ai_provider::LanguageModelV4ToolResult::new(
                                    &tc_id,
                                    "tool_search",
                                    result,
                                ),
                            ));
                    }
                    // Fallback materialization for tool calls that
                    // `*.done` deltas never closed (zero-arg / coalesced /
                    // terminal-`output_item.done`-only). If the delta-done
                    // already drained the active entry, `remove` returns
                    // `None` and this is a no-op — no double-emit.
                    ResponseOutputItem::FunctionCall { id, arguments, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        if let Some(mut fc) = self.active_fn_calls.remove(&item_id) {
                            if fc.arguments.is_empty()
                                && let Some(args) = arguments
                            {
                                fc.arguments = args.clone();
                            }
                            // `call_id` was seeded from `output_item.added`, so
                            // every emit already pairs to it — the terminal
                            // item carries no new correlation info.
                            self.finalize_fn_call(fc);
                        }
                    }
                    ResponseOutputItem::CustomToolCall { id, input, .. } => {
                        let item_id = id.clone().unwrap_or_default();
                        if let Some(mut ct) = self.active_custom_calls.remove(&item_id) {
                            if ct.input.is_empty()
                                && let Some(inp) = input
                            {
                                ct.input = inp.clone();
                            }
                            self.finalize_custom_call(ct);
                        }
                    }
                    // `output_item.done` is the only carrier of reasoning
                    // `encrypted_content` (the store=false chain-of-thought
                    // blob). Close the reasoning segment here — AFTER the
                    // summary text streamed — so the blob lands on the same
                    // accumulator segment. Mirrors codex materializing
                    // reasoning from `output_item.done`.
                    ResponseOutputItem::Reasoning {
                        id,
                        encrypted_content,
                        ..
                    } => {
                        let item_id = id.clone().unwrap_or_default();
                        let started = self
                            .active_reasoning
                            .remove(&item_id)
                            .map(|r| r.started)
                            .unwrap_or(false);
                        let meta = build_reasoning_provider_metadata(encrypted_content.as_ref());
                        if started {
                            // Close the summary segment, attaching the blob.
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ReasoningEnd {
                                    id: item_id,
                                    provider_metadata: meta,
                                });
                        } else if meta.is_some() {
                            // Encrypted-only reasoning (no summary streamed):
                            // open + close a segment so the chain round-trips.
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ReasoningStart {
                                    id: item_id.clone(),
                                    provider_metadata: None,
                                });
                            self.pending
                                .push_back(LanguageModelV4StreamPart::ReasoningEnd {
                                    id: item_id,
                                    provider_metadata: meta,
                                });
                        }
                    }
                    ResponseOutputItem::Compaction {
                        id,
                        encrypted_content,
                    } => {
                        let pm = build_compaction_provider_metadata(
                            id.as_deref().unwrap_or_default(),
                            encrypted_content.as_deref(),
                        );
                        self.pending.push_back(LanguageModelV4StreamPart::Custom {
                            kind: "openai-compaction".into(),
                            provider_metadata: Some(pm),
                        });
                    }
                    _ => {}
                }
            }

            ResponsesStreamEvent::Error { message, code } => {
                // The raw SSE payload is the only place the provider's real
                // failure detail reliably survives — `message`/`code` are
                // frequently null on server-side errors. Log it verbatim so
                // the failure is diagnosable without a wire capture, and fall
                // back to the raw text (not the opaque "Unknown error") when
                // both fields are absent.
                tracing::warn!(
                    code = code.as_deref().unwrap_or(""),
                    raw = %data,
                    "responses stream: error event",
                );
                let message = message
                    .or_else(|| {
                        code.clone()
                            .map(|c| format!("OpenAI responses error (code: {c})"))
                    })
                    .unwrap_or_else(|| format!("OpenAI responses error: {data}"));
                self.pending.push_back(LanguageModelV4StreamPart::Error {
                    error: vercel_ai_provider::StreamError {
                        message,
                        code,
                        is_retryable: false,
                    },
                });
            }

            _ => {}
        }
    }
}

/// Emit Source parts for annotations (url_citation, file_citation, file_path, container_file_citation).
fn emit_annotations(anns: &[ResponseAnnotation], content: &mut Vec<AssistantContentPart>) {
    for ann in anns {
        match ann {
            ResponseAnnotation::UrlCitation { url, title, .. } => {
                content.push(AssistantContentPart::Source(
                    vercel_ai_provider::content::SourcePart {
                        source_type: SourceType::Url,
                        id: vercel_ai_provider_utils::generate_id("src"),
                        url: url.clone(),
                        title: title.clone(),
                        media_type: None,
                        filename: None,
                        provider_metadata: None,
                    },
                ));
            }
            ResponseAnnotation::FileCitation { file_id, .. } => {
                content.push(AssistantContentPart::Source(
                    vercel_ai_provider::content::SourcePart {
                        source_type: SourceType::Document,
                        id: vercel_ai_provider_utils::generate_id("src"),
                        url: file_id.clone(),
                        title: None,
                        media_type: None,
                        filename: None,
                        provider_metadata: None,
                    },
                ));
            }
            ResponseAnnotation::FilePath { file_id, .. } => {
                content.push(AssistantContentPart::Source(
                    vercel_ai_provider::content::SourcePart {
                        source_type: SourceType::Document,
                        id: vercel_ai_provider_utils::generate_id("src"),
                        url: file_id.clone(),
                        title: None,
                        media_type: None,
                        filename: None,
                        provider_metadata: None,
                    },
                ));
            }
            ResponseAnnotation::ContainerFileCitation {
                file_id,
                container_id,
            } => {
                let mut meta_map = HashMap::new();
                if let Some(cid) = container_id {
                    meta_map.insert("containerId".into(), Value::String(cid.clone()));
                }
                let meta = if meta_map.is_empty() {
                    None
                } else {
                    Some(ProviderMetadata(meta_map))
                };
                content.push(AssistantContentPart::Source(
                    vercel_ai_provider::content::SourcePart {
                        source_type: SourceType::Document,
                        id: vercel_ai_provider_utils::generate_id("src"),
                        url: file_id.clone(),
                        title: None,
                        media_type: None,
                        filename: None,
                        provider_metadata: meta,
                    },
                ));
            }
            _ => {}
        }
    }
}

/// Ensure an entry exists in the `include` array, creating it if needed.
fn ensure_include_entry(body: &mut Value, entry: &str) {
    if body.get("include").is_none() {
        body["include"] = json!([]);
    }
    if let Some(arr) = body["include"].as_array_mut() {
        let val = Value::String(entry.into());
        if !arr.contains(&val) {
            arr.push(val);
        }
    }
}

fn set_optional_f32(body: &mut Value, key: &str, value: Option<f32>) {
    if let Some(v) = value {
        body[key] = json!(v);
    }
}

/// Local mirror of `coco_inference::PromptLayoutOptions`.
///
/// Provider crates do NOT depend on `coco-inference` (the SDK-fidelity
/// contract in `vercel-ai/provider/CLAUDE.md` forbids it). The wire
/// shape under `provider_options["prompt_layout"]` is the cross-layer
/// contract; this struct mirrors it locally so the consumer can parse
/// it with `serde_json::from_value`.
#[derive(serde::Deserialize, Default)]
struct PromptLayoutWire {
    #[serde(default)]
    instructions: Option<String>,
}

fn parse_prompt_layout_namespace(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> Option<PromptLayoutWire> {
    let opts = provider_options.as_ref()?;
    let inner = opts.get("prompt_layout")?;
    let mut object = serde_json::Map::new();
    for (key, value) in inner {
        object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(Value::Object(object)).ok()
}

#[cfg(test)]
#[path = "openai_responses_language_model.test.rs"]
mod tests;
