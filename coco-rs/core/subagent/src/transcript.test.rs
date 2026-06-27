use std::sync::Arc;
use uuid::Uuid;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::ReasoningPart;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultPart;
use coco_types::messages::AssistantMessage;
use coco_types::messages::Message;
use coco_types::messages::ToolResultMessage;
use coco_types::messages::UserMessage;

use super::*;

fn user(text: &str) -> Arc<Message> {
    Arc::new(Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    }))
}

fn assistant(parts: Vec<AssistantContentPart>) -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: parts,
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

fn tool_result(tool_use_id: &str) -> Arc<Message> {
    Arc::new(Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                tool_use_id,
                "Bash",
                ToolResultContent::text("ok"),
            ))],
            provider_options: None,
        },
        tool_use_id: tool_use_id.to_string(),
        tool_id: "Bash".parse().unwrap(),
        is_error: false,
    }))
}

#[test]
fn test_filter_whitespace_only_assistant() {
    let messages = vec![
        user("hello"),
        assistant(vec![AssistantContentPart::Text(TextPart::new("   \n  "))]),
        user("world"),
    ];

    let filtered = filter_transcript(&messages);
    assert_eq!(filtered.len(), 2);
    assert!(matches!(filtered[0].as_ref(), Message::User(_)));
    assert!(matches!(filtered[1].as_ref(), Message::User(_)));
}

#[test]
fn test_filter_thinking_only_assistant() {
    let messages = vec![
        user("hello"),
        assistant(vec![AssistantContentPart::Reasoning(ReasoningPart::new(
            "Let me think...",
        ))]),
        user("world"),
    ];

    let filtered = filter_transcript(&messages);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_filter_keeps_substantive_assistant() {
    let messages = vec![
        user("hello"),
        assistant(vec![
            AssistantContentPart::Reasoning(ReasoningPart::new("Let me think...")),
            AssistantContentPart::Text(TextPart::new("Here is my answer")),
        ]),
    ];

    let filtered = filter_transcript(&messages);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_filter_keeps_whitespace_text_plus_reasoning() {
    // A turn cancelled after emitting leading whitespace before a thinking
    // block has a non-text block, so it is NOT whitespace-only and must
    // survive. (Regression: the old predicate treated Reasoning as
    // "passing" and dropped it.)
    let messages = vec![
        user("hello"),
        assistant(vec![
            AssistantContentPart::Text(TextPart::new("  \n")),
            AssistantContentPart::Reasoning(ReasoningPart::new("partial thought")),
        ]),
        user("world"),
    ];

    let filtered = filter_transcript(&messages);
    assert_eq!(filtered.len(), 3);
    assert!(matches!(filtered[1].as_ref(), Message::Assistant(_)));
}

#[test]
fn test_strip_unresolved_tool_uses() {
    let messages = vec![
        user("hello"),
        assistant(vec![
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_1",
                "Bash",
                serde_json::Value::Null,
            )),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_2",
                "Read",
                serde_json::Value::Null,
            )),
        ]),
        // Only tu_1 resolved; tu_2 stays unresolved (this assistant
        // turn is NOT trailing so it isn't stripped — strip only
        // applies to the tail).
        tool_result("tu_1"),
        // Trailing assistant with unresolved tu_3 — must be stripped.
        assistant(vec![AssistantContentPart::ToolCall(ToolCallPart::new(
            "tu_3",
            "Write",
            serde_json::Value::Null,
        ))]),
    ];

    let filtered = filter_transcript(&messages);
    assert_eq!(filtered.len(), 3);
    assert!(matches!(filtered[2].as_ref(), Message::ToolResult(_)));
}

#[test]
fn test_filter_empty_transcript() {
    let messages: Vec<Arc<Message>> = vec![];
    let filtered = filter_transcript(&messages);
    assert!(filtered.is_empty());
}
