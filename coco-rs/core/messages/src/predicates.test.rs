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

/// Assistant message carrying one tool call per name in `tools`.
fn make_assistant_with_tools(tools: &[&str]) -> Message {
    let content = tools
        .iter()
        .map(|name| {
            AssistantContent::ToolCall(ToolCallContent::new(
                format!("call-{name}"),
                (*name).to_string(),
                serde_json::json!({}),
            ))
        })
        .collect();
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content,
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn messages_since_last_user_prompt_spans_whole_cycle() {
    let msgs = vec![
        make_user_msg("older prompt", false, false),
        make_assistant_msg(None),
        make_user_msg("current prompt", false, false),
        make_assistant_with_tools(&["Read"]),
        make_assistant_with_tools(&["Edit"]),
        make_assistant_msg(None),
    ];
    let cycle = messages_since_last_user_prompt(&msgs);
    assert_eq!(cycle.len(), 3);
}

#[test]
fn messages_since_last_user_prompt_ignores_virtual_and_compact_summary() {
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
        make_user_msg("real prompt", false, false),
        make_assistant_with_tools(&["Read"]),
        summary,
        make_user_msg("virtual", false, true),
        make_assistant_msg(None),
    ];
    // Engine bookkeeping must not open a new cycle, or the signal would be
    // computed over a slice that excludes the work the user actually asked for.
    let cycle = messages_since_last_user_prompt(&msgs);
    assert_eq!(cycle.len(), 4);
}

#[test]
fn messages_since_last_user_prompt_returns_all_when_no_prompt() {
    let msgs = vec![
        make_assistant_msg(None),
        make_assistant_with_tools(&["Read"]),
    ];
    assert_eq!(messages_since_last_user_prompt(&msgs).len(), 2);
}

#[test]
fn count_tool_calls_in_sums_across_the_cycle() {
    let msgs = vec![
        make_user_msg("prompt", false, false),
        make_assistant_with_tools(&["Read", "Grep"]),
        make_assistant_with_tools(&["Edit"]),
        make_assistant_msg(None),
    ];
    // The regression this guards: a tail-anchored count sees the trailing
    // text-only message and reports 0 for a 3-tool-call cycle.
    let cycle = messages_since_last_user_prompt(&msgs);
    assert_eq!(count_tool_calls_in(cycle), 3);
    assert_eq!(count_tool_calls_in_last_assistant_turn(cycle), 0);
}

#[test]
fn skill_invoked_in_detects_skill_before_a_text_only_ending() {
    let msgs = vec![
        make_user_msg("prompt", false, false),
        make_assistant_with_tools(&[coco_types::ToolName::Skill.as_str()]),
        make_assistant_with_tools(&["Read"]),
        make_assistant_msg(None),
    ];
    let cycle = messages_since_last_user_prompt(&msgs);
    assert!(skill_invoked_in(cycle));
}

#[test]
fn skill_invoked_in_is_false_without_a_skill_call() {
    let msgs = vec![
        make_user_msg("prompt", false, false),
        make_assistant_with_tools(&["Read"]),
        make_assistant_msg(None),
    ];
    let cycle = messages_since_last_user_prompt(&msgs);
    assert!(!skill_invoked_in(cycle));
}
