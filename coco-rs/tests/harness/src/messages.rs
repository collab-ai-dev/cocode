//! Unified message builders for tests.
//!
//! Consolidates 22 duplicated builder functions across 15 test files into
//! a single canonical set. Import as `use coco_test_harness::messages as msg;`.

use coco_messages::AssistantContent;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageKind;
use coco_messages::StopReason;
use coco_messages::TombstoneMessage;
use coco_messages::ToolContent;
use coco_messages::ToolResultContent;
use coco_messages::ToolResultMessage;
use coco_messages::UserContent;
use coco_messages::UserMessage;
use coco_types::ToolId;
use coco_types::ToolName;
use uuid::Uuid;

// ── User messages ───────────────────────────────────────────────────

/// User message with text content.
pub fn user(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

/// Meta user message (hidden from UI, visible to model).
/// Lands as `Message::Attachment` (kind = CriticalSystemReminder) — same
/// behavior as `coco_messages::create_meta_message`.
pub fn user_meta(text: &str) -> Message {
    Message::Attachment(coco_messages::AttachmentMessage::api(
        coco_types::AttachmentKind::CriticalSystemReminder,
        LlmMessage::user_text(text),
    ))
}

/// User message with an image file part (for image stripping tests).
pub fn image_user() -> Message {
    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![
                UserContent::Text(coco_llm_types::TextPart::new("see this image")),
                UserContent::File(coco_llm_types::FilePart {
                    data: coco_llm_types::SharedV4FileData::data_base64("iVBORw0KGgo="),
                    media_type: "image/png".to_string(),
                    filename: Some("test.png".to_string()),
                    provider_metadata: None,
                }),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

// ── Assistant messages ──────────────────────────────────────────────

/// Assistant message with text (random UUID).
pub fn assistant(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant_text(text),
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Assistant message with a specific UUID (for grouping tests).
pub fn assistant_with_uuid(text: &str, uuid: Uuid) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant_text(text),
        uuid,
        model: "test-model".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Assistant message with reasoning/thinking blocks.
pub fn assistant_with_thinking(text: &str, thinking: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![
                AssistantContent::Reasoning(coco_llm_types::ReasoningPart {
                    text: thinking.to_string(),
                    provider_metadata: None,
                }),
                AssistantContent::Text(coco_llm_types::TextPart::new(text)),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Assistant message with a tool call.
pub fn assistant_with_tool_call(tool_name: &str, input: serde_json::Value) -> Message {
    assistant_with_tool_call_id(&format!("call_{tool_name}"), tool_name, input)
}

/// Assistant message with a tool call and explicit tool-use id.
pub fn assistant_with_tool_call_id(
    tool_call_id: &str,
    tool_name: &str,
    input: serde_json::Value,
) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![
                AssistantContent::Text(coco_llm_types::TextPart::new(format!("Using {tool_name}"))),
                AssistantContent::ToolCall(coco_llm_types::ToolCallPart {
                    tool_call_id: tool_call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    input,
                    provider_executed: None,
                    provider_metadata: None,
                    invalid: false,
                    invalid_reason: None,
                }),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

// ── Tool result messages ────────────────────────────────────────────

/// Tool result message with parameterized tool, id, and content.
pub fn tool_result(tool: ToolName, tool_use_id: &str, content: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(ToolResultContent {
                tool_call_id: tool_use_id.to_string(),
                tool_name: tool.as_str().to_string(),
                output: coco_llm_types::ToolResultContent::text(content),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: tool_use_id.to_string(),
        tool_id: ToolId::Builtin(tool),
        is_error: false,
    })
}

/// Large tool result for token-controlled tests.
pub fn tool_result_large(tool: ToolName, tool_use_id: &str, size_chars: usize) -> Message {
    tool_result(tool, tool_use_id, &"x".repeat(size_chars))
}

// ── System messages ─────────────────────────────────────────────────

/// Tombstone message (placeholder for deleted message).
pub fn tombstone() -> Message {
    Message::Tombstone(TombstoneMessage {
        uuid: Uuid::new_v4(),
        original_kind: MessageKind::ToolResult,
    })
}
