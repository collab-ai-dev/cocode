use crate::AssistantContent;
use crate::INTERRUPT_MESSAGE;
use crate::INTERRUPT_MESSAGE_FOR_TOOL_USE;
use crate::LlmMessage;
use crate::Message;
use crate::MessageKind;
use crate::MessageOrigin;
use crate::SystemMessage;
use crate::SystemMessageLevel;
use coco_types::TokenUsage;
use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_create_user_message() {
    let msg = create_user_message("hello");
    let Message::User(u) = &msg else {
        panic!("expected User variant");
    };
    assert_eq!(msg.kind(), MessageKind::User);
    assert_eq!(u.origin, Some(MessageOrigin::UserInput));
    assert!(msg.uuid().is_some());
}

#[test]
fn test_create_meta_message() {
    // Post-Phase-2: meta messages land as Message::Attachment with kind.
    let msg = create_meta_message("system context");
    let Message::Attachment(a) = &msg else {
        panic!("expected Attachment variant");
    };
    assert_eq!(a.kind, coco_types::AttachmentKind::CriticalSystemReminder);
}

#[test]
fn test_create_info_message() {
    let msg = create_info_message("Title", "body text");
    let Message::System(SystemMessage::Informational(info)) = &msg else {
        panic!("expected SystemMessage::Informational");
    };
    assert_eq!(info.title, "Title");
    assert_eq!(info.message, "body text");
    assert_eq!(info.level, SystemMessageLevel::Info);
}

#[test]
fn test_create_assistant_message() {
    let content = vec![AssistantContent::text("response")];
    let usage = TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 10,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 20,
            ..Default::default()
        },
    };
    let msg = create_assistant_message(content, "gpt-4", usage);
    let Message::Assistant(a) = &msg else {
        panic!("expected Assistant variant");
    };
    assert_eq!(a.model, "gpt-4");
    assert_eq!(a.usage, Some(usage));
    assert!(a.api_error.is_none());
    assert!(matches!(a.message, LlmMessage::Assistant { .. }));
}

#[test]
fn test_create_tool_result_message_success() {
    let tool_id = ToolId::Builtin(ToolName::Read);
    let msg = create_tool_result_message("call_1", "Read", tool_id.clone(), "file contents", false);
    let Message::ToolResult(tr) = &msg else {
        panic!("expected ToolResult variant");
    };
    assert_eq!(tr.tool_use_id, "call_1");
    assert_eq!(tr.tool_id, tool_id);
    assert!(!tr.is_error);
    assert!(matches!(tr.message, LlmMessage::Tool { .. }));
}

#[test]
fn test_create_tool_result_message_error() {
    let tool_id = ToolId::Builtin(ToolName::Bash);
    let msg = create_tool_result_message("call_2", "Bash", tool_id, "command failed", true);
    let Message::ToolResult(tr) = &msg else {
        panic!("expected ToolResult variant");
    };
    assert!(tr.is_error);
}

#[test]
fn test_create_error_tool_result() {
    let tool_id = ToolId::Builtin(ToolName::Write);
    let msg = create_error_tool_result("call_3", "Write", tool_id, "permission denied");
    let Message::ToolResult(tr) = &msg else {
        panic!("expected ToolResult variant");
    };
    assert!(tr.is_error);
    assert_eq!(tr.tool_use_id, "call_3");
}

#[test]
fn test_create_compact_boundary_message() {
    let msg = create_compact_boundary_message(50000, 20000);
    let Message::System(SystemMessage::CompactBoundary(cb)) = &msg else {
        panic!("expected CompactBoundary variant");
    };
    assert_eq!(cb.tokens_before, 50000);
    assert_eq!(cb.tokens_after, 20000);
}

#[test]
fn test_create_progress_message() {
    let data = serde_json::json!({"progress": 50});
    let msg = create_progress_message("tool_1", data.clone());
    let Message::Progress(p) = &msg else {
        panic!("expected Progress variant");
    };
    assert_eq!(p.tool_use_id, "tool_1");
    assert_eq!(p.data, data);
    assert!(p.parent_message_uuid.is_none());
}

#[test]
fn user_interruption_message_uses_canonical_text() {
    // The interruption message text must match one of `INTERRUPT_MESSAGE` /
    // `INTERRUPT_MESSAGE_FOR_TOOL_USE` byte-for-byte.
    let plain = create_user_interruption_message(/*for_tool_use*/ false);
    let Message::User(u) = &plain else {
        panic!("expected Message::User");
    };
    let coco_llm_types::LlmMessage::User { content, .. } = &u.message else {
        panic!("expected LlmMessage::User");
    };
    let [coco_llm_types::UserContentPart::Text(text)] = content.as_slice() else {
        panic!("expected single text content part");
    };
    assert_eq!(text.text, INTERRUPT_MESSAGE);

    let tool = create_user_interruption_message(/*for_tool_use*/ true);
    let Message::User(u) = &tool else {
        panic!("expected Message::User");
    };
    let coco_llm_types::LlmMessage::User { content, .. } = &u.message else {
        panic!("expected LlmMessage::User");
    };
    let [coco_llm_types::UserContentPart::Text(text)] = content.as_slice() else {
        panic!("expected single text content part");
    };
    assert_eq!(text.text, INTERRUPT_MESSAGE_FOR_TOOL_USE);
}

#[test]
fn test_create_permission_denied_message() {
    let msg = create_permission_denied_message("Bash", "not allowed in sandbox");
    let Message::System(SystemMessage::Informational(info)) = &msg else {
        panic!("expected SystemMessage::Informational");
    };
    assert_eq!(info.level, SystemMessageLevel::Warning);
    assert_eq!(info.title, "Permission denied: Bash");
    assert_eq!(info.message, "not allowed in sandbox");
}

#[test]
fn test_create_assistant_error_message_with_request_id() {
    let msg = create_assistant_error_message("rate limited", Some("req_abc"), Some("rate_limited"));
    let Message::Assistant(a) = &msg else {
        panic!("expected Assistant variant");
    };
    assert_eq!(a.request_id.as_deref(), Some("req_abc"));
    let err = a.api_error.as_ref().expect("should have api_error");
    assert_eq!(err.message, "rate limited");
    assert!(err.status_code.is_none());
    assert_eq!(err.error_type.as_deref(), Some("rate_limited"));
}

#[test]
fn test_create_assistant_error_message_without_request_id() {
    let msg = create_assistant_error_message("unknown error", None, None);
    let Message::Assistant(a) = &msg else {
        panic!("expected Assistant variant");
    };
    assert!(a.request_id.is_none());
    let err = a.api_error.as_ref().expect("should have api_error");
    assert!(err.error_type.is_none());
    assert!(a.usage.is_none());
    assert!(matches!(a.message, LlmMessage::Assistant { .. }));
}

/// Pin the legacy-text interrupt markers to their canonical literals.
///
/// `last_message_is_user_interruption` (in `coco_query::history_sync`)
/// dedups across the legacy-text form so older JSONL transcripts
/// resume correctly. If these literals ever drift, dedup silently
/// breaks on imported transcripts.
#[test]
fn interrupt_message_constants_match_ts_literals() {
    assert_eq!(INTERRUPT_MESSAGE, "[Request interrupted by user]");
    assert_eq!(
        INTERRUPT_MESSAGE_FOR_TOOL_USE,
        "[Request interrupted by user for tool use]"
    );
}
