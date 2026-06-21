use std::collections::HashMap;
use std::collections::HashSet;

use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::Warning;

use crate::cache_control::CacheContext;
use crate::cache_control::CacheControlValidator;

/// Result of converting a prompt to Anthropic messages.
pub struct ConvertedMessages {
    pub system: Option<Vec<Value>>,
    pub messages: Vec<Value>,
    pub warnings: Vec<Warning>,
    pub betas: HashSet<String>,
}

// ---------------------------------------------------------------------------
// Message block grouping (port of TS groupIntoBlocks)
// ---------------------------------------------------------------------------

/// Block types for grouping consecutive messages.
enum MessageBlock<'a> {
    System(Vec<&'a LanguageModelV4Message>),
    User(Vec<&'a LanguageModelV4Message>),
    Assistant(Vec<&'a LanguageModelV4Message>),
}

/// The block category for grouping purposes.
#[derive(PartialEq)]
enum BlockType {
    System,
    User,
    Assistant,
}

/// Group consecutive messages of the same role category.
/// Tool messages are grouped with User messages (both become role: "user").
fn group_into_blocks(prompt: &LanguageModelV4Prompt) -> Vec<MessageBlock<'_>> {
    let mut blocks: Vec<MessageBlock<'_>> = Vec::new();
    let mut current_type: Option<BlockType> = None;

    for msg in prompt {
        let msg_type = match msg {
            LanguageModelV4Message::System { .. } | LanguageModelV4Message::Developer { .. } => {
                BlockType::System
            }
            LanguageModelV4Message::User { .. } | LanguageModelV4Message::Tool { .. } => {
                BlockType::User
            }
            LanguageModelV4Message::Assistant { .. } => BlockType::Assistant,
        };

        let same_block = current_type.as_ref() == Some(&msg_type);

        if same_block {
            // Append to current block
            match blocks.last_mut() {
                Some(MessageBlock::System(msgs)) => msgs.push(msg),
                Some(MessageBlock::User(msgs)) => msgs.push(msg),
                Some(MessageBlock::Assistant(msgs)) => msgs.push(msg),
                None => unreachable!(),
            }
        } else {
            // Start new block
            current_type = Some(msg_type);
            match msg {
                LanguageModelV4Message::System { .. }
                | LanguageModelV4Message::Developer { .. } => {
                    blocks.push(MessageBlock::System(vec![msg]));
                }
                LanguageModelV4Message::User { .. } | LanguageModelV4Message::Tool { .. } => {
                    blocks.push(MessageBlock::User(vec![msg]));
                }
                LanguageModelV4Message::Assistant { .. } => {
                    blocks.push(MessageBlock::Assistant(vec![msg]));
                }
            }
        }
    }

    blocks
}

// ---------------------------------------------------------------------------
// Tool name mapping (SDK tool ID ↔ provider API name)
// ---------------------------------------------------------------------------

/// Bidirectional tool name mapping.
///
/// - `to_provider`: maps SDK tool ID → provider API name
///   (e.g. "anthropic.code_execution_20260120" → "code_execution")
/// - `to_sdk`: maps provider API name → SDK tool ID
///   (e.g. "code_execution" → "anthropic.code_execution_20260120")
pub struct ToolNameMapping {
    to_provider: HashMap<String, String>,
    to_sdk: HashMap<String, String>,
}

impl ToolNameMapping {
    pub fn new(api_to_sdk: &HashMap<String, String>) -> Self {
        let mut to_provider = HashMap::new();
        for (api_name, sdk_id) in api_to_sdk {
            to_provider.insert(sdk_id.clone(), api_name.clone());
        }
        Self {
            to_provider,
            to_sdk: api_to_sdk.clone(),
        }
    }

    pub fn empty() -> Self {
        Self {
            to_provider: HashMap::new(),
            to_sdk: HashMap::new(),
        }
    }

    /// Map SDK tool name to provider API name.
    /// Returns the original name if no mapping exists.
    pub fn to_provider_tool_name<'a>(&'a self, sdk_name: &'a str) -> &'a str {
        self.to_provider
            .get(sdk_name)
            .map(String::as_str)
            .unwrap_or(sdk_name)
    }

    /// Map provider API name to SDK tool ID.
    /// Returns the original name if no mapping exists.
    pub fn to_sdk_tool_name<'a>(&'a self, api_name: &'a str) -> &'a str {
        self.to_sdk
            .get(api_name)
            .map(String::as_str)
            .unwrap_or(api_name)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a `LanguageModelV4Prompt` into Anthropic Messages API format.
///
/// Returns `(system, messages, warnings)` where system is a separate array of
/// text blocks (Anthropic uses a top-level `system` field, not a system role message).
pub fn convert_to_anthropic_messages(
    prompt: &LanguageModelV4Prompt,
    send_reasoning: bool,
) -> (Option<Vec<Value>>, Vec<Value>, Vec<Warning>) {
    let result = convert_to_anthropic_messages_full(
        prompt,
        send_reasoning,
        &ToolNameMapping::empty(),
        &mut CacheControlValidator::new(),
    );
    (result.system, result.messages, result.warnings)
}

/// Convert prompt with full result including betas, cache control, and tool name mapping.
pub fn convert_to_anthropic_messages_full(
    prompt: &LanguageModelV4Prompt,
    send_reasoning: bool,
    tool_name_mapping: &ToolNameMapping,
    cache_validator: &mut CacheControlValidator,
) -> ConvertedMessages {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let mut betas: HashSet<String> = HashSet::new();

    let blocks = group_into_blocks(prompt);
    let num_blocks = blocks.len();
    let mut system_seen = false;

    for (block_idx, block) in blocks.into_iter().enumerate() {
        let is_last_block = block_idx == num_blocks - 1;

        match block {
            MessageBlock::System(msgs) => {
                if system_seen {
                    warnings.push(Warning::Unsupported {
                        feature:
                            "Multiple system messages that are separated by user/assistant messages"
                                .into(),
                        details: None,
                    });
                }
                system_seen = true;
                for msg in msgs {
                    if let LanguageModelV4Message::System {
                        content,
                        provider_options,
                    }
                    | LanguageModelV4Message::Developer {
                        content,
                        provider_options,
                    } = msg
                    {
                        let cache_control = cache_validator.get_cache_control_from_options(
                            provider_options,
                            CacheContext {
                                type_name: "system message",
                                can_cache: true,
                            },
                        );
                        let text =
                            collapse_text_parts(content, &mut warnings, "system/developer message");
                        let mut block = json!({
                            "type": "text",
                            "text": text,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        system_blocks.push(block);
                    }
                }
            }

            MessageBlock::User(msgs) => {
                // Combine all user and tool messages in this block into a single message
                let mut anthropic_content: Vec<Value> = Vec::new();

                for msg in &msgs {
                    match msg {
                        LanguageModelV4Message::User {
                            content,
                            provider_options: msg_provider_options,
                        } => {
                            let num_parts = content.len();
                            for (j, part) in content.iter().enumerate() {
                                let is_last_part = j == num_parts - 1;
                                let cache_control = get_part_cache_control(
                                    cache_validator,
                                    part,
                                    is_last_part,
                                    msg_provider_options,
                                    "user message part",
                                    "user message",
                                );

                                convert_user_part(
                                    part,
                                    cache_control,
                                    &mut anthropic_content,
                                    &mut betas,
                                    &mut warnings,
                                );
                            }
                        }
                        LanguageModelV4Message::Tool {
                            content,
                            provider_options: msg_provider_options,
                        } => {
                            let num_parts = content.len();
                            for (j, part) in content.iter().enumerate() {
                                let is_last_part = j == num_parts - 1;
                                let cache_control = get_tool_part_cache_control(
                                    cache_validator,
                                    part,
                                    is_last_part,
                                    msg_provider_options,
                                );

                                convert_tool_part(
                                    part,
                                    cache_control,
                                    &mut anthropic_content,
                                    &mut warnings,
                                    &mut betas,
                                );
                            }
                        }
                        _ => {} // System/Assistant can't appear in a User block
                    }
                }

                if !anthropic_content.is_empty() {
                    messages.push(json!({
                        "role": "user",
                        "content": anthropic_content,
                    }));
                }
            }

            MessageBlock::Assistant(msgs) => {
                // Combine all assistant messages in this block into a single message
                let mut anthropic_content: Vec<Value> = Vec::new();
                let mut mcp_tool_use_ids: HashSet<String> = HashSet::new();
                let num_msgs = msgs.len();

                for (msg_idx, msg) in msgs.iter().enumerate() {
                    let is_last_message = msg_idx == num_msgs - 1;

                    if let LanguageModelV4Message::Assistant {
                        content,
                        provider_options: msg_provider_options,
                    } = msg
                    {
                        let num_parts = content.len();
                        for (k, part) in content.iter().enumerate() {
                            let is_last_content_part = k == num_parts - 1;
                            let cache_control = get_assistant_part_cache_control(
                                cache_validator,
                                part,
                                is_last_content_part,
                                msg_provider_options,
                            );

                            convert_assistant_part(
                                part,
                                send_reasoning,
                                tool_name_mapping,
                                cache_control,
                                is_last_block && is_last_message && is_last_content_part,
                                &mut mcp_tool_use_ids,
                                &mut anthropic_content,
                                &mut warnings,
                            );
                        }
                    }
                }

                if !anthropic_content.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": anthropic_content,
                    }));
                }
            }
        }
    }

    let system = if system_blocks.is_empty() {
        None
    } else {
        Some(system_blocks)
    };

    ConvertedMessages {
        system,
        messages,
        warnings,
        betas,
    }
}

fn collapse_text_parts(
    parts: &[UserContentPart],
    warnings: &mut Vec<Warning>,
    context: &str,
) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(text_part) => text.push_str(&text_part.text),
            UserContentPart::File(_) => warnings.push(Warning::unsupported_with_details(
                "non-text prompt part",
                format!("{context} contains a non-text part that was dropped"),
            )),
        }
    }
    text
}

// ---------------------------------------------------------------------------
// Cache control helpers
// ---------------------------------------------------------------------------

/// Get cache control for a user content part, falling back to message-level
/// cache control on the last part.
fn get_part_cache_control(
    validator: &mut CacheControlValidator,
    part: &UserContentPart,
    is_last_part: bool,
    msg_provider_options: &Option<ProviderOptions>,
    part_context: &str,
    msg_context: &str,
) -> Option<Value> {
    let part_pm = match part {
        UserContentPart::Text(tp) => &tp.provider_metadata,
        UserContentPart::File(fp) => &fp.provider_metadata,
    };

    validator
        .get_cache_control(
            part_pm,
            CacheContext {
                type_name: part_context,
                can_cache: true,
            },
        )
        .or_else(|| {
            if is_last_part {
                validator.get_cache_control_from_options(
                    msg_provider_options,
                    CacheContext {
                        type_name: msg_context,
                        can_cache: true,
                    },
                )
            } else {
                None
            }
        })
}

/// Get cache control for a tool content part.
fn get_tool_part_cache_control(
    validator: &mut CacheControlValidator,
    part: &ToolContentPart,
    is_last_part: bool,
    msg_provider_options: &Option<ProviderOptions>,
) -> Option<Value> {
    let part_pm = match part {
        ToolContentPart::ToolResult(tr) => &tr.provider_metadata,
        ToolContentPart::ToolApprovalResponse(_) => &None,
    };

    validator
        .get_cache_control(
            part_pm,
            CacheContext {
                type_name: "tool result part",
                can_cache: true,
            },
        )
        .or_else(|| {
            if is_last_part {
                validator.get_cache_control_from_options(
                    msg_provider_options,
                    CacheContext {
                        type_name: "tool result message",
                        can_cache: true,
                    },
                )
            } else {
                None
            }
        })
}

/// Get cache control for an assistant content part.
fn get_assistant_part_cache_control(
    validator: &mut CacheControlValidator,
    part: &AssistantContentPart,
    is_last_content_part: bool,
    msg_provider_options: &Option<ProviderOptions>,
) -> Option<Value> {
    let (part_pm, can_cache) = match part {
        AssistantContentPart::Text(tp) => (&tp.provider_metadata, true),
        AssistantContentPart::ToolCall(tc) => (&tc.provider_metadata, true),
        // Thinking/redacted_thinking blocks cannot have cache_control directly.
        // They are cached implicitly in previous assistant turns.
        AssistantContentPart::Reasoning(rp) => (&rp.provider_metadata, false),
        AssistantContentPart::ToolResult(tr) => (&tr.provider_metadata, true),
        _ => (&None, true),
    };

    validator
        .get_cache_control(
            part_pm,
            CacheContext {
                type_name: "assistant message part",
                can_cache,
            },
        )
        .or_else(|| {
            if is_last_content_part {
                validator.get_cache_control_from_options(
                    msg_provider_options,
                    CacheContext {
                        type_name: "assistant message",
                        can_cache: true,
                    },
                )
            } else {
                None
            }
        })
}

// ---------------------------------------------------------------------------
// User part conversion
// ---------------------------------------------------------------------------

/// Convert a single user content part to Anthropic format.
fn convert_user_part(
    part: &UserContentPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    betas: &mut HashSet<String>,
    warnings: &mut Vec<Warning>,
) {
    match part {
        UserContentPart::Text(text_part) => {
            let mut block = json!({
                "type": "text",
                "text": text_part.text,
            });
            if let Some(cc) = cache_control {
                block["cache_control"] = cc;
            }
            result.push(block);
        }
        UserContentPart::File(file_part) => {
            let media_type = &file_part.media_type;
            if media_type.starts_with("image/") {
                // Image content — map image/* to image/jpeg
                let actual_media_type = if media_type == "image/*" {
                    "image/jpeg"
                } else {
                    media_type
                };
                let source = data_content_to_anthropic_source(&file_part.data, actual_media_type);
                let mut block = json!({
                    "type": "image",
                    "source": source,
                });
                if let Some(cc) = cache_control {
                    block["cache_control"] = cc;
                }
                result.push(block);
            } else if media_type == "application/pdf" {
                // PDF document
                betas.insert("pdfs-2024-09-25".to_string());
                let source = data_content_to_anthropic_source(&file_part.data, media_type);
                let mut doc = json!({
                    "type": "document",
                    "source": source,
                });
                // Extract provider options for PDF (citations, title, context)
                if let Some(ref pm) = file_part.provider_metadata
                    && let Some(opts) = pm.0.get("anthropic")
                {
                    if let Some(citations) = opts.get("citations") {
                        doc["citations"] = citations.clone();
                    }
                    if let Some(title) = opts.get("title") {
                        doc["title"] = title.clone();
                    }
                    if let Some(context) = opts.get("context") {
                        doc["context"] = context.clone();
                    }
                }
                if let Some(cc) = cache_control {
                    doc["cache_control"] = cc;
                }
                result.push(doc);
            } else if media_type == "text/plain" {
                // Plain text document — with citations support
                let source = data_content_to_text_source(&file_part.data, media_type);
                let mut doc = json!({
                    "type": "document",
                    "source": source,
                });
                if let Some(ref pm) = file_part.provider_metadata
                    && let Some(opts) = pm.0.get("anthropic")
                {
                    if let Some(citations) = opts.get("citations") {
                        doc["citations"] = citations.clone();
                    }
                    if let Some(title) = opts.get("title") {
                        doc["title"] = title.clone();
                    }
                    if let Some(context) = opts.get("context") {
                        doc["context"] = context.clone();
                    }
                }
                if let Some(cc) = cache_control {
                    doc["cache_control"] = cc;
                }
                result.push(doc);
            } else if media_type.starts_with("text/") {
                // Other text/* documents
                let source = data_content_to_text_source(&file_part.data, media_type);
                let mut doc = json!({
                    "type": "document",
                    "source": source,
                });
                if let Some(cc) = cache_control {
                    doc["cache_control"] = cc;
                }
                result.push(doc);
            } else {
                // TS only supports image/*, application/pdf, text/plain.
                // text/* is a Rust extension above. Anything else → warning.
                warnings.push(Warning::Unsupported {
                    feature: format!("media type: {media_type}"),
                    details: None,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool part conversion (in user blocks)
// ---------------------------------------------------------------------------

/// Convert a single tool content part to Anthropic `tool_result` block.
fn convert_tool_part(
    part: &ToolContentPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
    betas: &mut HashSet<String>,
) {
    match part {
        ToolContentPart::ToolResult(tool_result) => {
            let (content, is_error) = serialize_tool_result(&tool_result.output, warnings, betas);
            let mut block = json!({
                "type": "tool_result",
                "tool_use_id": tool_result.tool_call_id,
                "content": content,
            });
            if is_error {
                block["is_error"] = Value::Bool(true);
            }
            if let Some(cc) = cache_control {
                block["cache_control"] = cc;
            }
            result.push(block);
        }
        ToolContentPart::ToolApprovalResponse(_) => {
            warnings.push(Warning::Unsupported {
                feature: "tool approval responses".into(),
                details: Some(
                    "Tool approval responses are not supported in Anthropic Messages API".into(),
                ),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Assistant part conversion (with provider-executed tools and round-trip)
// ---------------------------------------------------------------------------

/// Convert a single assistant content part to Anthropic format.
///
/// Handles text, reasoning, tool calls (including provider-executed),
/// tool results (for round-trip), and compaction blocks.
#[allow(clippy::too_many_arguments)]
fn convert_assistant_part(
    part: &AssistantContentPart,
    send_reasoning: bool,
    tool_name_mapping: &ToolNameMapping,
    cache_control: Option<Value>,
    is_trailing_part: bool,
    mcp_tool_use_ids: &mut HashSet<String>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    match part {
        AssistantContentPart::Text(text_part) => {
            // Check for compaction block via provider metadata
            let is_compaction = text_part
                .provider_metadata
                .as_ref()
                .and_then(|pm| pm.0.get("anthropic"))
                .and_then(|v| v.get("type"))
                .and_then(|t| t.as_str())
                == Some("compaction");

            if is_compaction {
                let mut block = json!({
                    "type": "compaction",
                    "content": text_part.text,
                });
                if let Some(cc) = cache_control {
                    block["cache_control"] = cc;
                }
                result.push(block);
            } else {
                // Trim trailing whitespace on the last text part of the last
                // assistant block (Anthropic requirement for pre-filled responses)
                let text = if is_trailing_part {
                    text_part.text.trim().to_string()
                } else {
                    text_part.text.clone()
                };
                let mut block = json!({
                    "type": "text",
                    "text": text,
                });
                if let Some(cc) = cache_control {
                    block["cache_control"] = cc;
                }
                result.push(block);
            }
        }

        AssistantContentPart::ToolCall(tc) => {
            if tc.provider_executed == Some(true) {
                // Provider-executed tool call
                let provider_tool_name = tool_name_mapping.to_provider_tool_name(&tc.tool_name);

                // Check if MCP tool use
                let is_mcp_tool_use = tc
                    .provider_metadata
                    .as_ref()
                    .and_then(|pm| pm.0.get("anthropic"))
                    .and_then(|v| v.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("mcp-tool-use");

                if is_mcp_tool_use {
                    mcp_tool_use_ids.insert(tc.tool_call_id.clone());

                    let server_name = tc
                        .provider_metadata
                        .as_ref()
                        .and_then(|pm| pm.0.get("anthropic"))
                        .and_then(|v| v.get("serverName"))
                        .and_then(|s| s.as_str());

                    if let Some(server_name) = server_name {
                        let mut block = json!({
                            "type": "mcp_tool_use",
                            "id": tc.tool_call_id,
                            "name": tc.tool_name,
                            "input": tc.input,
                            "server_name": server_name,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        result.push(block);
                    } else {
                        warnings.push(Warning::Other {
                            message: "mcp tool use server name is required and must be a string"
                                .into(),
                        });
                    }
                } else if provider_tool_name == "code_execution" {
                    // Code execution sub-tool handling
                    let input_type = tc.input.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    if input_type == "bash_code_execution"
                        || input_type == "text_editor_code_execution"
                    {
                        // Map back to sub-tool name
                        let mut block = json!({
                            "type": "server_tool_use",
                            "id": tc.tool_call_id,
                            "name": input_type,
                            "input": tc.input,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        result.push(block);
                    } else if input_type == "programmatic-tool-call" {
                        // Strip the fake type before sending to Anthropic
                        let mut input_without_type = tc.input.clone();
                        if let Some(obj) = input_without_type.as_object_mut() {
                            obj.remove("type");
                        }
                        let mut block = json!({
                            "type": "server_tool_use",
                            "id": tc.tool_call_id,
                            "name": "code_execution",
                            "input": input_without_type,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        result.push(block);
                    } else {
                        // Standard code_execution server tool use
                        let mut block = json!({
                            "type": "server_tool_use",
                            "id": tc.tool_call_id,
                            "name": provider_tool_name,
                            "input": tc.input,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        result.push(block);
                    }
                } else if provider_tool_name == "web_fetch"
                    || provider_tool_name == "web_search"
                    || provider_tool_name == "tool_search_tool_regex"
                    || provider_tool_name == "tool_search_tool_bm25"
                {
                    let mut block = json!({
                        "type": "server_tool_use",
                        "id": tc.tool_call_id,
                        "name": provider_tool_name,
                        "input": tc.input,
                    });
                    if let Some(cc) = cache_control {
                        block["cache_control"] = cc;
                    }
                    result.push(block);
                } else {
                    warnings.push(Warning::Other {
                        message: format!(
                            "provider executed tool call for tool {} is not supported",
                            tc.tool_name
                        ),
                    });
                }
            } else {
                // Regular (non-provider-executed) tool call
                let mut block = json!({
                    "type": "tool_use",
                    "id": tc.tool_call_id,
                    "name": tc.tool_name,
                    "input": tc.input,
                });

                // Extract caller info from provider options for programmatic tool calling
                if let Some(ref pm) = tc.provider_metadata
                    && let Some(anthropic) = pm.0.get("anthropic")
                    && let Some(caller) = anthropic.get("caller")
                {
                    // Forward caller as-is (camelCase to snake_case mapping for toolId → tool_id)
                    let caller_type = caller.get("type").and_then(|t| t.as_str());
                    let caller_tool_id = caller
                        .get("toolId")
                        .or_else(|| caller.get("tool_id"))
                        .and_then(|t| t.as_str());

                    if let Some(ct) = caller_type {
                        let mut caller_val = json!({"type": ct});
                        if let Some(tid) = caller_tool_id {
                            caller_val["tool_id"] = Value::String(tid.to_string());
                        }
                        block["caller"] = caller_val;
                    }
                }

                if let Some(cc) = cache_control {
                    block["cache_control"] = cc;
                }
                result.push(block);
            }
        }

        AssistantContentPart::Reasoning(reasoning) => {
            if send_reasoning {
                let anthropic_meta = reasoning
                    .provider_metadata
                    .as_ref()
                    .and_then(|pm| pm.0.get("anthropic"));

                let has_signature = anthropic_meta
                    .and_then(|v| v.get("signature"))
                    .and_then(|v| v.as_str())
                    .is_some();
                let has_redacted = anthropic_meta.and_then(|v| v.get("redactedData")).is_some();

                if has_signature {
                    let signature = anthropic_meta
                        .and_then(|v| v.get("signature"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    result.push(json!({
                        "type": "thinking",
                        "thinking": reasoning.text,
                        "signature": signature,
                    }));
                } else if has_redacted {
                    let data = anthropic_meta
                        .and_then(|v| v.get("redactedData"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    result.push(json!({
                        "type": "redacted_thinking",
                        "data": data,
                    }));
                } else {
                    // No signature or redactedData — unsupported reasoning metadata
                    warnings.push(Warning::Other {
                        message: "unsupported reasoning metadata".into(),
                    });
                }
            } else {
                // Reasoning content is disabled for this model
                warnings.push(Warning::Other {
                    message: "sending reasoning content is disabled for this model".into(),
                });
            }
        }

        AssistantContentPart::ToolResult(tr) => {
            // Tool results in assistant blocks (round-trip for provider-executed tools)
            let provider_tool_name = tool_name_mapping.to_provider_tool_name(&tr.tool_name);

            if mcp_tool_use_ids.contains(&tr.tool_call_id) {
                // MCP tool result
                match &tr.output {
                    ToolResultContent::Json { value, .. }
                    | ToolResultContent::ErrorJson { value, .. } => {
                        let is_error = matches!(&tr.output, ToolResultContent::ErrorJson { .. });
                        let mut block = json!({
                            "type": "mcp_tool_result",
                            "tool_use_id": tr.tool_call_id,
                            "is_error": is_error,
                            "content": value,
                        });
                        if let Some(cc) = cache_control {
                            block["cache_control"] = cc;
                        }
                        result.push(block);
                    }
                    _ => {
                        warnings.push(Warning::Other {
                            message: format!(
                                "provider executed tool result output type for tool {} is not supported",
                                tr.tool_name
                            ),
                        });
                    }
                }
            } else if provider_tool_name == "code_execution" {
                convert_code_execution_tool_result(tr, cache_control, result, warnings);
            } else if provider_tool_name == "web_fetch" {
                convert_web_fetch_tool_result(tr, cache_control, result, warnings);
            } else if provider_tool_name == "web_search" {
                convert_web_search_tool_result(tr, cache_control, result, warnings);
            } else if provider_tool_name == "tool_search_tool_regex"
                || provider_tool_name == "tool_search_tool_bm25"
            {
                convert_tool_search_tool_result(tr, cache_control, result, warnings);
            } else {
                warnings.push(Warning::Other {
                    message: format!(
                        "provider executed tool result for tool {} is not supported",
                        tr.tool_name
                    ),
                });
            }
        }

        // Source, File, ToolApprovalRequest — skip
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Provider-executed tool result converters (round-trip)
// ---------------------------------------------------------------------------

/// Convert a code execution tool result for round-trip.
fn convert_code_execution_tool_result(
    tr: &vercel_ai_provider::content::ToolResultPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    // Handle error types
    let is_error = matches!(
        &tr.output,
        ToolResultContent::ErrorText { .. } | ToolResultContent::ErrorJson { .. }
    );
    if is_error {
        let error_obj: Value = match &tr.output {
            ToolResultContent::ErrorJson { value, .. } => value.clone(),
            ToolResultContent::ErrorText { value, .. } => {
                serde_json::from_str(value).unwrap_or(json!({}))
            }
            _ => json!({}),
        };

        let error_type = error_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let error_code = error_obj
            .get("errorCode")
            .or_else(|| error_obj.get("error_code"))
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");

        if error_type == "code_execution_tool_result_error" {
            let mut block = json!({
                "type": "code_execution_tool_result",
                "tool_use_id": tr.tool_call_id,
                "content": {
                    "type": "code_execution_tool_result_error",
                    "error_code": error_code,
                },
            });
            if let Some(cc) = cache_control {
                block["cache_control"] = cc;
            }
            result.push(block);
        } else {
            let mut block = json!({
                "type": "bash_code_execution_tool_result",
                "tool_use_id": tr.tool_call_id,
                "content": {
                    "type": "bash_code_execution_tool_result_error",
                    "error_code": error_code,
                },
            });
            if let Some(cc) = cache_control {
                block["cache_control"] = cc;
            }
            result.push(block);
        }
        return;
    }

    let ToolResultContent::Json { value, .. } = &tr.output else {
        warnings.push(Warning::Other {
            message: format!(
                "provider executed tool result output type for tool {} is not supported",
                tr.tool_name
            ),
        });
        return;
    };

    let result_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

    if result_type.is_empty() {
        warnings.push(Warning::Other {
            message: format!(
                "provider executed tool result output value is not a valid code execution result for tool {}",
                tr.tool_name
            ),
        });
        return;
    }

    if result_type == "code_execution_result" || result_type == "encrypted_code_execution_result" {
        // code_execution 20250522 or 20260120
        // Ensure content field defaults to [] if absent/null (matches TS Zod .default([]))
        let mut content_value = value.clone();
        if let Some(obj) = content_value.as_object_mut()
            && (!obj.contains_key("content") || obj["content"].is_null())
        {
            obj.insert("content".into(), json!([]));
        }
        let mut block = json!({
            "type": "code_execution_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": content_value,
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
    } else if result_type == "bash_code_execution_result"
        || result_type == "bash_code_execution_tool_result_error"
    {
        let mut block = json!({
            "type": "bash_code_execution_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": value,
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
    } else {
        // text_editor_code_execution_result or other
        let mut block = json!({
            "type": "text_editor_code_execution_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": value,
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
    }
}

/// Convert a web_fetch tool result for round-trip.
fn convert_web_fetch_tool_result(
    tr: &vercel_ai_provider::content::ToolResultPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    // Handle error types
    if let ToolResultContent::ErrorJson { value, .. } = &tr.output {
        let error_code = value
            .get("errorCode")
            .or_else(|| value.get("error_code"))
            .and_then(|c| c.as_str())
            .unwrap_or("unavailable");

        let mut block = json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": {
                "type": "web_fetch_tool_result_error",
                "error_code": error_code,
            },
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
        return;
    }

    let ToolResultContent::Json { value, .. } = &tr.output else {
        warnings.push(Warning::Other {
            message: format!(
                "provider executed tool result output type for tool {} is not supported",
                tr.tool_name
            ),
        });
        return;
    };

    // Transform camelCase fields to snake_case and build proper nested structure.
    // Required fields: url, retrievedAt, content.source.type, content.source.mediaType
    let url = value.get("url").and_then(|v| v.as_str());
    let retrieved_at = value
        .get("retrievedAt")
        .or_else(|| value.get("retrieved_at"))
        .and_then(|v| v.as_str());
    let content_obj = value.get("content");
    let source = content_obj.and_then(|c| c.get("source"));
    let source_type = source.and_then(|s| s.get("type")).and_then(|t| t.as_str());
    let source_media_type = source
        .and_then(|s| s.get("mediaType").or_else(|| s.get("media_type")))
        .and_then(|t| t.as_str());

    if let (Some(url), Some(retrieved_at), Some(source_type), Some(source_media_type)) =
        (url, retrieved_at, source_type, source_media_type)
    {
        let mut source_val = json!({
            "type": source_type,
            "media_type": source_media_type,
        });
        if let Some(data) = source.and_then(|s| s.get("data")) {
            source_val["data"] = data.clone();
        }

        let mut doc_content = json!({
            "type": "document",
            "source": source_val,
        });
        if let Some(title) = content_obj.and_then(|c| c.get("title")) {
            doc_content["title"] = title.clone();
        }
        if let Some(citations) = content_obj.and_then(|c| c.get("citations")) {
            doc_content["citations"] = citations.clone();
        }

        let mut block = json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": {
                "type": "web_fetch_result",
                "url": url,
                "retrieved_at": retrieved_at,
                "content": doc_content,
            },
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
    } else {
        // Fallback: forward raw JSON if required fields missing
        warnings.push(Warning::Other {
            message: format!(
                "web_fetch tool result missing required fields for tool {}, forwarding raw JSON",
                tr.tool_name
            ),
        });
        let mut block = json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": tr.tool_call_id,
            "content": value,
        });
        if let Some(cc) = cache_control {
            block["cache_control"] = cc;
        }
        result.push(block);
    }
}

/// Convert a web_search tool result for round-trip.
/// Transforms camelCase fields: `pageAge` → `page_age`, `encryptedContent` → `encrypted_content`.
fn convert_web_search_tool_result(
    tr: &vercel_ai_provider::content::ToolResultPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    let ToolResultContent::Json { value, .. } = &tr.output else {
        warnings.push(Warning::Other {
            message: format!(
                "provider executed tool result output type for tool {} is not supported",
                tr.tool_name
            ),
        });
        return;
    };

    // Transform array elements: pageAge → page_age, encryptedContent → encrypted_content
    let transformed_content = if let Some(arr) = value.as_array() {
        let items: Vec<Value> = arr
            .iter()
            .map(|item| {
                let mut out = json!({});
                if let Some(url) = item.get("url") {
                    out["url"] = url.clone();
                }
                if let Some(title) = item.get("title") {
                    out["title"] = title.clone();
                }
                if let Some(t) = item.get("type") {
                    out["type"] = t.clone();
                }
                if let Some(page_age) = item.get("pageAge").or_else(|| item.get("page_age")) {
                    out["page_age"] = page_age.clone();
                }
                if let Some(enc) = item
                    .get("encryptedContent")
                    .or_else(|| item.get("encrypted_content"))
                {
                    out["encrypted_content"] = enc.clone();
                }
                out
            })
            .collect();
        Value::Array(items)
    } else {
        // Not an array — forward as-is
        value.clone()
    };

    let mut block = json!({
        "type": "web_search_tool_result",
        "tool_use_id": tr.tool_call_id,
        "content": transformed_content,
    });
    if let Some(cc) = cache_control {
        block["cache_control"] = cc;
    }
    result.push(block);
}

/// Convert a tool_search tool result for round-trip.
/// Transforms to structured format: `{ type: "tool_search_tool_search_result", tool_references: [...] }`.
fn convert_tool_search_tool_result(
    tr: &vercel_ai_provider::content::ToolResultPart,
    cache_control: Option<Value>,
    result: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    let ToolResultContent::Json { value, .. } = &tr.output else {
        warnings.push(Warning::Other {
            message: format!(
                "provider executed tool result output type for tool {} is not supported",
                tr.tool_name
            ),
        });
        return;
    };

    // Transform: array of {toolName: "..."} → { type: "tool_search_tool_search_result", tool_references: [{type: "tool_reference", tool_name: "..."}] }
    let tool_references = if let Some(arr) = value.as_array() {
        arr.iter()
            .filter_map(|item| {
                let tool_name = item
                    .get("toolName")
                    .or_else(|| item.get("tool_name"))
                    .and_then(|v| v.as_str());
                tool_name.map(|name| {
                    json!({
                        "type": "tool_reference",
                        "tool_name": name,
                    })
                })
            })
            .collect::<Vec<Value>>()
    } else {
        Vec::new()
    };

    let mut block = json!({
        "type": "tool_search_tool_result",
        "tool_use_id": tr.tool_call_id,
        "content": {
            "type": "tool_search_tool_search_result",
            "tool_references": tool_references,
        },
    });
    if let Some(cc) = cache_control {
        block["cache_control"] = cc;
    }
    result.push(block);
}

// ---------------------------------------------------------------------------
// Tool result serialization (for user-block tool results)
// ---------------------------------------------------------------------------

/// Serialize tool result content into `(Value, is_error)`.
fn serialize_tool_result(
    content: &ToolResultContent,
    warnings: &mut Vec<Warning>,
    betas: &mut HashSet<String>,
) -> (Value, bool) {
    match content {
        ToolResultContent::Text { value, .. } => (Value::String(value.clone()), false),
        ToolResultContent::Json { value, .. } => (
            Value::String(serde_json::to_string(value).unwrap_or_default()),
            false,
        ),
        ToolResultContent::ErrorText { value, .. } => (Value::String(value.clone()), true),
        ToolResultContent::ErrorJson { value, .. } => (
            Value::String(serde_json::to_string(value).unwrap_or_default()),
            true,
        ),
        ToolResultContent::ExecutionDenied { reason, .. } => {
            let msg = reason
                .clone()
                .unwrap_or_else(|| "Tool execution denied.".into());
            (Value::String(msg), true)
        }
        ToolResultContent::Content { value, .. } => {
            let parts: Vec<Value> = value
                .iter()
                .filter_map(|part| convert_tool_result_content_part(part, warnings, betas))
                .collect();
            (Value::Array(parts), false)
        }
    }
}

/// Convert a single `ToolResultContentPart` to an Anthropic content block.
fn convert_tool_result_content_part(
    part: &vercel_ai_provider::ToolResultContentPart,
    warnings: &mut Vec<Warning>,
    betas: &mut HashSet<String>,
) -> Option<Value> {
    match part {
        vercel_ai_provider::ToolResultContentPart::Text { text, .. } => {
            Some(json!({"type": "text", "text": text}))
        }
        vercel_ai_provider::ToolResultContentPart::FileUrl {
            url, media_type, ..
        } => {
            if media_type.starts_with("image/") || media_type == "image" {
                Some(json!({
                    "type": "image",
                    "source": { "type": "url", "url": url }
                }))
            } else {
                Some(json!({
                    "type": "document",
                    "source": { "type": "url", "url": url }
                }))
            }
        }
        vercel_ai_provider::ToolResultContentPart::FileReference { .. } => {
            warnings.push(Warning::Other {
                message: "Anthropic does not accept tool-result file-reference parts".into(),
            });
            None
        }
        vercel_ai_provider::ToolResultContentPart::FileData {
            data, media_type, ..
        } => {
            if media_type == "application/pdf" {
                betas.insert("pdfs-2024-09-25".to_string());
                Some(json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                }))
            } else if media_type.starts_with("image/") {
                // Image file data
                Some(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                }))
            } else {
                warnings.push(Warning::Other {
                    message: format!(
                        "unsupported tool content part type: file-data with media type: {media_type}",
                    ),
                });
                None
            }
        }
        vercel_ai_provider::ToolResultContentPart::Custom {
            provider_options, ..
        } => {
            let anthropic = provider_options
                .as_ref()
                .and_then(|po| po.0.get("anthropic"));
            let anthropic_type = anthropic
                .and_then(|v| v.get("type"))
                .and_then(|t| t.as_str());

            if anthropic_type == Some("tool-reference") {
                let tool_name = anthropic
                    .and_then(|v| v.get("toolName").or_else(|| v.get("tool_name")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some(json!({
                    "type": "tool_reference",
                    "tool_name": tool_name,
                }))
            } else {
                warnings.push(Warning::Other {
                    message: "unsupported custom tool content part".into(),
                });
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Data content conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `FileRawData` (bytes or base64) to a base64 string.
fn file_raw_data_to_base64(raw: &FileRawData) -> String {
    match raw {
        FileRawData::Base64(b64) => b64.clone(),
        FileRawData::Bytes(bytes) => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(bytes)
        }
    }
}

/// Convert `SharedV4FileData` to an Anthropic source object (`base64` or `url`).
fn data_content_to_anthropic_source(data: &SharedV4FileData, media_type: &str) -> Value {
    match data {
        SharedV4FileData::Url { url } => {
            json!({ "type": "url", "url": url })
        }
        SharedV4FileData::Data { data: raw } => {
            let b64 = file_raw_data_to_base64(raw);
            json!({ "type": "base64", "media_type": media_type, "data": b64 })
        }
        SharedV4FileData::Text { text } => {
            json!({ "type": "text", "media_type": media_type, "data": text })
        }
        SharedV4FileData::Reference { reference } => {
            // Provider references are not supported as Anthropic sources in this context;
            // return a placeholder that will surface as an API error.
            json!({ "type": "reference", "reference": reference })
        }
    }
}

/// Convert `SharedV4FileData` to a text source for text/* documents.
fn data_content_to_text_source(data: &SharedV4FileData, media_type: &str) -> Value {
    match data {
        SharedV4FileData::Text { text } => {
            json!({ "type": "text", "media_type": media_type, "data": text })
        }
        SharedV4FileData::Data { data: raw } => match raw {
            FileRawData::Base64(b64) => {
                use base64::Engine as _;
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
                    && let Ok(text) = String::from_utf8(bytes)
                {
                    return json!({ "type": "text", "media_type": media_type, "data": text });
                }
                json!({ "type": "base64", "media_type": media_type, "data": b64 })
            }
            FileRawData::Bytes(bytes) => {
                if let Ok(text) = String::from_utf8(bytes.clone()) {
                    json!({ "type": "text", "media_type": media_type, "data": text })
                } else {
                    use base64::Engine as _;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                    json!({ "type": "base64", "media_type": media_type, "data": b64 })
                }
            }
        },
        SharedV4FileData::Url { url } => {
            json!({ "type": "url", "url": url })
        }
        SharedV4FileData::Reference { reference } => {
            json!({ "type": "reference", "reference": reference })
        }
    }
}

#[cfg(test)]
#[path = "convert_to_anthropic_messages.test.rs"]
mod tests;
