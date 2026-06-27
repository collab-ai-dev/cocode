use crate::*;
use uuid::Uuid;

use super::*;

fn make_user_msg(text: &str, meta: bool, virtual_flag: bool) -> Message {
    // Post-Phase-2: meta=true → Message::Attachment; meta=false → regular User.
    if meta {
        Message::Attachment(crate::AttachmentMessage::api(
            coco_types::AttachmentKind::CriticalSystemReminder,
            LlmMessage::user_text(text),
        ))
    } else {
        Message::User(UserMessage {
            message: LlmMessage::user_text(text),
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            is_visible_in_transcript_only: false,
            is_virtual: virtual_flag,
            is_compact_summary: false,
            permission_mode: None,
            origin: None,
            parent_tool_use_id: None,
        })
    }
}

fn make_assistant_msg(stop: Option<StopReason>) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: "hello".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: stop,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn test_is_user_message() {
    let msg = make_user_msg("hi", false, false);
    assert!(is_user_message(&msg));
    assert!(!is_assistant_message(&msg));
}

#[test]
fn test_is_meta_message() {
    let meta = make_user_msg("system", true, false);
    let normal = make_user_msg("user", false, false);
    assert!(is_meta_message(&meta));
    assert!(!is_meta_message(&normal));
}

#[test]
fn test_is_virtual_message() {
    let virtual_msg = make_user_msg("ghost", false, true);
    assert!(is_virtual_message(&virtual_msg));
}

#[test]
fn test_stopped_for_tool_use() {
    let msg = make_assistant_msg(Some(StopReason::ToolUse));
    assert!(stopped_for_tool_use(&msg));
    assert!(!stopped_for_max_tokens(&msg));
}

#[test]
fn test_stopped_for_max_tokens() {
    let msg = make_assistant_msg(Some(StopReason::MaxTokens));
    assert!(stopped_for_max_tokens(&msg));
    assert!(!stopped_for_tool_use(&msg));
}

#[test]
fn test_has_text_content() {
    let msg = make_user_msg("hello", false, false);
    assert!(has_text_content(&msg));

    let tombstone = Message::Tombstone(TombstoneMessage {
        uuid: Uuid::new_v4(),
        original_kind: MessageKind::User,
    });
    assert!(!has_text_content(&tombstone));
}

#[test]
fn messages_after_compact_boundary_returns_all_when_no_boundary() {
    let msgs = vec![
        make_user_msg("a", false, false),
        make_assistant_msg(None),
        make_user_msg("b", false, false),
    ];
    let after = messages_after_compact_boundary(&msgs);
    assert_eq!(after.len(), 3);
}

#[test]
fn messages_after_compact_boundary_slices_at_boundary_marker() {
    let boundary = create_compact_boundary_message(50_000, 12_000);
    let msgs = vec![
        make_user_msg("before", false, false),
        make_assistant_msg(None),
        boundary,
        make_user_msg("after-1", false, false),
        make_assistant_msg(None),
    ];
    let after = messages_after_compact_boundary(&msgs);
    assert_eq!(after.len(), 2);
    assert!(matches!(&after[0], Message::User(u) if matches!(
        &u.message,
        LlmMessage::User { content, .. } if content.iter().any(|c| matches!(c, UserContent::Text(t) if t.text == "after-1"))
    )));
}

#[test]
fn messages_after_compact_boundary_slices_at_compact_summary_user() {
    let summary = Message::User(UserMessage {
        message: LlmMessage::user_text("[summary]"),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: true,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    });
    let msgs = vec![
        make_user_msg("pre", false, false),
        summary,
        make_user_msg("post", false, false),
    ];
    let after = messages_after_compact_boundary(&msgs);
    assert_eq!(after.len(), 1);
}

#[test]
fn messages_after_compact_boundary_uses_most_recent() {
    let b1 = create_compact_boundary_message(10, 1);
    let b2 = create_compact_boundary_message(20, 2);
    let msgs = vec![
        make_user_msg("a", false, false),
        b1,
        make_user_msg("b", false, false),
        b2,
        make_user_msg("c", false, false),
        make_user_msg("d", false, false),
    ];
    let after = messages_after_compact_boundary(&msgs);
    assert_eq!(after.len(), 2);
}
