use super::TranscriptEntry;
use super::TranscriptUsage;
use super::entry_kind;
use coco_types::ToolId;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::str::FromStr;
use uuid::Uuid;

/// Context shared by transcript entries generated for one message.
#[derive(Debug, Clone, Copy)]
pub struct TranscriptEntryOptions<'a> {
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub timestamp: &'a str,
    pub parent_uuid: Option<&'a str>,
    pub logical_parent_uuid: Option<&'a str>,
    pub is_sidechain: bool,
    pub agent_id: Option<&'a str>,
    /// Current git branch for `cwd`, captured once per chain by the
    /// caller and stamped on every transcript line. `None` ⇒ field is
    /// omitted on serialize.
    pub git_branch: Option<&'a str>,
}

/// Convert an internal message to one or more transcript message entries.
pub fn transcript_entries_for_message(
    msg: &coco_messages::Message,
    options: TranscriptEntryOptions<'_>,
) -> Vec<TranscriptEntry> {
    let Some(uuid) = msg.uuid().copied() else {
        return Vec::new();
    };
    let (entry_type, message_value, model, usage, cost_usd) = match msg {
        coco_messages::Message::User(u) => (
            entry_kind::USER,
            serde_json::to_value(&u.message).ok(),
            None,
            None,
            None,
        ),
        coco_messages::Message::Assistant(a) => {
            let usage = a.usage.as_ref().map(|u| TranscriptUsage {
                input_tokens: u.input_tokens.total,
                output_tokens: u.output_tokens.total,
                cache_read_tokens: Some(u.input_tokens.cache_read),
                cache_creation_tokens: Some(u.input_tokens.cache_write),
            });
            (
                entry_kind::ASSISTANT,
                serde_json::to_value(&a.message).ok(),
                Some(a.model.clone()).filter(|m| !m.is_empty()),
                usage,
                a.cost_usd,
            )
        }
        coco_messages::Message::System(s) => (
            entry_kind::SYSTEM,
            serde_json::to_value(s).ok(),
            None,
            None,
            None,
        ),
        coco_messages::Message::Attachment(att) => (
            entry_kind::ATTACHMENT,
            // Serialise the whole AttachmentMessage so kind + body +
            // extras round-trip together. Earlier revisions persisted
            // only `att.body` and lost the `kind` discriminator on
            // read. This entry MUST be readable.
            serde_json::to_value(att).ok(),
            None,
            None,
            None,
        ),
        coco_messages::Message::ToolResult(t) => (
            entry_kind::USER,
            Some(tool_result_to_ts_user_message(t)),
            None,
            None,
            None,
        ),
        coco_messages::Message::Progress(_) | coco_messages::Message::Tombstone(_) => {
            return Vec::new();
        }
    };

    vec![TranscriptEntry {
        entry_type: entry_type.to_string(),
        uuid: uuid.to_string(),
        parent_uuid: options.parent_uuid.map(str::to_string),
        logical_parent_uuid: options
            .logical_parent_uuid
            .filter(|_| options.parent_uuid.is_none())
            .map(str::to_string),
        session_id: options.session_id.to_string(),
        cwd: options.cwd.to_string(),
        timestamp: options.timestamp.to_string(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        git_branch: options.git_branch.map(str::to_string),
        is_sidechain: options.is_sidechain,
        agent_id: options.agent_id.map(str::to_string),
        message: message_value,
        usage,
        model,
        cost_usd,
        extra: user_envelope_extra(msg),
    }]
}

/// Persist the `Message::User` envelope flags that don't live inside the
/// inner `LlmMessage` but must survive a JSONL round-trip. Critically,
/// `is_visible_in_transcript_only` gates model-visibility: a slash-command
/// echo/result with `display: system` would otherwise resume as a
/// model-visible user message (an API leak on resume). `origin` is kept so
/// the command-pill renderer's classification is stable across resume.
/// Only non-default values are written, so ordinary user turns serialize
/// byte-for-byte as before.
fn user_envelope_extra(msg: &coco_messages::Message) -> serde_json::Map<String, serde_json::Value> {
    let mut extra = serde_json::Map::new();
    if let coco_messages::Message::User(u) = msg {
        if u.is_visible_in_transcript_only {
            extra.insert("is_visible_in_transcript_only".to_string(), json!(true));
        }
        if let Some(origin) = u.origin
            && origin != coco_messages::MessageOrigin::UserInput
            && let Ok(value) = serde_json::to_value(origin)
        {
            extra.insert("origin".to_string(), value);
        }
    }
    extra
}

pub(super) fn is_compact_boundary_message(msg: &coco_messages::Message) -> bool {
    matches!(
        msg,
        coco_messages::Message::System(coco_messages::SystemMessage::CompactBoundary(_))
            | coco_messages::Message::System(coco_messages::SystemMessage::MicrocompactBoundary(_))
    )
}

pub(super) fn remember_assistant_tool_calls(
    msg: &coco_messages::Message,
    assistant_uuid: Uuid,
    out: &mut std::collections::HashMap<String, Uuid>,
) {
    let coco_messages::Message::Assistant(assistant) = msg else {
        return;
    };
    let coco_messages::LlmMessage::Assistant { content, .. } = &assistant.message else {
        return;
    };
    for part in content {
        if let coco_messages::AssistantContent::ToolCall(call) = part {
            out.insert(call.tool_call_id.clone(), assistant_uuid);
        }
    }
}

pub(super) fn source_assistant_uuid_for_tool_result(msg: &coco_messages::Message) -> Option<Uuid> {
    let coco_messages::Message::ToolResult(result) = msg else {
        return None;
    };
    result.source_assistant_uuid
}

pub(super) fn tool_result_use_id(msg: &coco_messages::Message) -> Option<&str> {
    let coco_messages::Message::ToolResult(result) = msg else {
        return None;
    };
    Some(result.tool_use_id.as_str())
}

fn tool_result_to_ts_user_message(tr: &coco_messages::ToolResultMessage) -> serde_json::Value {
    let content = match &tr.message {
        coco_messages::LlmMessage::Tool { content, .. } => content
            .iter()
            .filter_map(|part| match part {
                coco_messages::ToolContent::ToolResult(result) => {
                    Some(tool_result_part_to_ts_block(result, tr.is_error))
                }
                coco_messages::ToolContent::ToolApprovalResponse(_) => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    json!({
        "role": "user",
        "content": content,
    })
}

fn tool_result_part_to_ts_block(
    result: &coco_messages::ToolResultContent,
    fallback_is_error: bool,
) -> serde_json::Value {
    let mut block = serde_json::Map::new();
    block.insert("type".to_string(), json!("tool_result"));
    block.insert("tool_use_id".to_string(), json!(result.tool_call_id));
    block.insert("tool_name".to_string(), json!(result.tool_name));
    let is_error = result.is_error || fallback_is_error;
    if is_error {
        block.insert("is_error".to_string(), json!(true));
    }
    block.insert(
        "content".to_string(),
        tool_result_output_to_ts_content(&result.output),
    );
    serde_json::Value::Object(block)
}

fn tool_result_output_to_ts_content(output: &coco_messages::ToolResultOutput) -> serde_json::Value {
    match output {
        coco_messages::ToolResultOutput::Text { value, .. }
        | coco_messages::ToolResultOutput::ErrorText { value, .. } => json!(value),
        coco_messages::ToolResultOutput::Json { value, .. }
        | coco_messages::ToolResultOutput::ErrorJson { value, .. } => value.clone(),
        coco_messages::ToolResultOutput::ExecutionDenied { reason, .. } => {
            json!(
                reason
                    .clone()
                    .unwrap_or_else(|| "Execution denied".to_string())
            )
        }
        coco_messages::ToolResultOutput::Content { value, .. } => {
            serde_json::to_value(value).unwrap_or_else(|_| json!(""))
        }
    }
}

/// Reconstruct zero or more internal messages from a transcript entry.
pub fn messages_from_transcript_entry(entry: &TranscriptEntry) -> Vec<coco_messages::Message> {
    if entry.entry_type == entry_kind::USER
        && let Some(messages) = tool_results_from_ts_user_entry(entry)
        && !messages.is_empty()
    {
        return messages;
    }
    reconstruct_regular_message(entry).into_iter().collect()
}

fn reconstruct_regular_message(entry: &TranscriptEntry) -> Option<coco_messages::Message> {
    let uuid = entry
        .uuid
        .parse::<Uuid>()
        .unwrap_or_else(|_| Uuid::new_v4());
    let msg_value = entry.message.clone()?;

    match entry.entry_type.as_str() {
        entry_kind::USER => {
            let llm = serde_json::from_value::<coco_messages::LlmMessage>(msg_value.clone())
                .unwrap_or_else(|_| {
                    coco_messages::LlmMessage::user_text(extract_text_from_json_content(
                        msg_value.get("content").unwrap_or(&serde_json::Value::Null),
                    ))
                });
            Some(coco_messages::Message::User(coco_messages::UserMessage {
                message: llm,
                uuid,
                timestamp: entry.timestamp.clone(),
                // Restore the model-visibility gate (see `user_envelope_extra`).
                is_visible_in_transcript_only: entry
                    .extra
                    .get("is_visible_in_transcript_only")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                is_virtual: false,
                is_compact_summary: false,
                permission_mode: None,
                origin: entry
                    .extra
                    .get("origin")
                    .and_then(|v| serde_json::from_value(v.clone()).ok()),
                parent_tool_use_id: None,
            }))
        }
        entry_kind::ASSISTANT => {
            let llm = serde_json::from_value::<coco_messages::LlmMessage>(msg_value.clone())
                .unwrap_or_else(|_| {
                    coco_messages::LlmMessage::assistant_text(extract_text_from_json_content(
                        msg_value.get("content").unwrap_or(&serde_json::Value::Null),
                    ))
                });
            let usage = entry.usage.as_ref().map(|u| coco_types::TokenUsage {
                input_tokens: coco_types::InputTokens {
                    total: u.input_tokens,
                    cache_read: u.cache_read_tokens.unwrap_or(0),
                    cache_write: u.cache_creation_tokens.unwrap_or(0),
                    ..Default::default()
                },
                output_tokens: coco_types::OutputTokens {
                    total: u.output_tokens,
                    ..Default::default()
                },
            });
            Some(coco_messages::Message::Assistant(
                coco_messages::AssistantMessage {
                    message: llm,
                    uuid,
                    model: entry.model.clone().unwrap_or_default(),
                    stop_reason: None,
                    usage,
                    cost_usd: entry.cost_usd,
                    request_id: None,
                    api_error: None,
                },
            ))
        }
        entry_kind::SYSTEM => tag_and_deserialize_message(msg_value.clone(), entry_kind::SYSTEM)
            .or_else(|| {
                let llm = coco_messages::LlmMessage::user_text(extract_text_from_json_content(
                    &msg_value,
                ));
                let mut att = coco_messages::AttachmentMessage::api(
                    coco_types::AttachmentKind::CriticalSystemReminder,
                    llm,
                );
                att.uuid = uuid;
                Some(coco_messages::Message::Attachment(att))
            }),
        entry_kind::ATTACHMENT => {
            // Symmetric counterpart to the write-side
            // `serde_json::to_value(att)` — the JSON carries the full
            // AttachmentMessage shape (uuid + kind + body + extras).
            // Stamp our `entry.uuid` over the deserialised uuid so
            // identity stays anchored to the JSONL row even if the
            // serialised AttachmentMessage was authored elsewhere.
            let mut att =
                serde_json::from_value::<coco_messages::AttachmentMessage>(msg_value).ok()?;
            att.uuid = uuid;
            Some(coco_messages::Message::Attachment(att))
        }
        _ => None,
    }
}

fn tool_results_from_ts_user_entry(entry: &TranscriptEntry) -> Option<Vec<coco_messages::Message>> {
    let content = entry.message.as_ref()?.get("content")?.as_array()?;
    let mut messages = Vec::new();
    let multiple_blocks = content
        .iter()
        .filter(|block| {
            block
                .get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|kind| kind == "tool_result" || kind == "tool-result")
        })
        .count()
        > 1;
    let source_assistant_uuid = entry
        .parent_uuid
        .as_deref()
        .and_then(|uuid| uuid.parse::<Uuid>().ok());
    for (idx, block) in content.iter().enumerate() {
        let Some(kind) = block.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if kind != "tool_result" && kind != "tool-result" {
            continue;
        }
        let tool_use_id = block
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if tool_use_id.is_empty() {
            continue;
        }
        let tool_name = block
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let is_error = block
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let output = ts_tool_result_content_to_output(
            block
                .get("content")
                .or_else(|| block.get("output"))
                .unwrap_or(&serde_json::Value::Null),
            is_error,
        );
        let part = coco_messages::ToolResultContent {
            tool_call_id: tool_use_id.clone(),
            tool_name: tool_name.clone(),
            output,
            is_error,
            provider_metadata: None,
        };
        let uuid = if multiple_blocks {
            deterministic_tool_result_block_uuid(&entry.uuid, &tool_use_id, idx)
        } else {
            entry.uuid.parse::<Uuid>().unwrap_or_else(|_| {
                deterministic_tool_result_block_uuid(&entry.uuid, &tool_use_id, idx)
            })
        };
        let tool_id = match ToolId::from_str(&tool_name) {
            Ok(tool_id) => tool_id,
            Err(never) => match never {},
        };
        messages.push(coco_messages::Message::ToolResult(
            coco_messages::ToolResultMessage {
                uuid,
                source_assistant_uuid,
                display_data: None,
                message: coco_messages::LlmMessage::tool(vec![
                    coco_messages::ToolContent::ToolResult(part),
                ]),
                tool_use_id,
                tool_id,
                is_error,
            },
        ));
    }
    Some(messages)
}

fn deterministic_tool_result_block_uuid(entry_uuid: &str, tool_use_id: &str, idx: usize) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(entry_uuid.as_bytes());
    hasher.update([0]);
    hasher.update(tool_use_id.as_bytes());
    hasher.update([0]);
    hasher.update(idx.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn ts_tool_result_content_to_output(
    content: &serde_json::Value,
    is_error: bool,
) -> coco_messages::ToolResultOutput {
    if let Some(text) = content.as_str() {
        return if is_error {
            coco_messages::ToolResultOutput::error_text(text)
        } else {
            coco_messages::ToolResultOutput::text(text)
        };
    }
    if let Some(parts) = content.as_array() {
        let parsed = parts
            .iter()
            .filter_map(|part| {
                serde_json::from_value::<coco_messages::ToolResultContentPart>(part.clone()).ok()
            })
            .collect::<Vec<_>>();
        if !parsed.is_empty() {
            return coco_messages::ToolResultOutput::content_parts(parsed);
        }
    }
    if is_error {
        coco_messages::ToolResultOutput::error_json(content.clone())
    } else {
        coco_messages::ToolResultOutput::json(content.clone())
    }
}

fn tag_and_deserialize_message(
    payload: serde_json::Value,
    kind: &str,
) -> Option<coco_messages::Message> {
    let mut map = match payload {
        serde_json::Value::Object(m) => m,
        _ => return None,
    };
    map.insert(
        "type".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    serde_json::from_value::<coco_messages::Message>(serde_json::Value::Object(map)).ok()
}

fn extract_text_from_json_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) == Some("text")
                && let Some(text) = item.get("text").and_then(|v| v.as_str())
            {
                parts.push(text);
            }
        }
        return parts.join("\n");
    }
    String::new()
}
