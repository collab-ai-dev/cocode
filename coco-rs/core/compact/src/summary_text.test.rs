use coco_messages::*;
use uuid::Uuid;

use super::*;

fn make_user(text: &str) -> Message {
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

fn make_assistant(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant_text(text),
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn test_estimate_tokens_text() {
    // 400 chars → ~100 tokens
    let msg = make_user(&"a".repeat(400));
    let tokens = estimate_message_tokens(&msg);
    assert_eq!(tokens, 100);
}

#[test]
fn test_estimate_tokens_empty() {
    let msgs: Vec<Message> = vec![];
    assert_eq!(estimate_tokens_for_messages(&msgs), 0);
}

#[test]
fn test_extract_message_text_user() {
    let msg = make_user("hello world");
    let text = extract_message_text(&msg).unwrap();
    assert_eq!(text, "hello world");
}

#[test]
fn test_extract_message_text_assistant() {
    let msg = make_assistant("response text");
    let text = extract_message_text(&msg).unwrap();
    assert_eq!(text, "response text");
}

#[test]
fn test_conservative_estimate_padded() {
    let msgs = vec![make_user(&"x".repeat(300))];
    let base = estimate_tokens_for_messages(&msgs);
    let conservative = coco_messages::estimate_tokens_for_messages_conservative(&msgs);
    assert!(conservative > base, "conservative should be larger");
    // 300/4 = 75, * 4/3 = 100
    assert_eq!(base, 75);
    assert_eq!(conservative, 100);
}
