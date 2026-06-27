use crate::*;
use coco_types::InputTokens;
use coco_types::OutputTokens;
use coco_types::ProviderModelSelection;
use coco_types::TokenUsage;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use uuid::Uuid;

use super::*;

fn user_msg(text: &str) -> Message {
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

fn assistant_msg(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn assistant_msg_empty() -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![],
            provider_options: None,
        },
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
fn test_len_and_is_empty() {
    let mut history = MessageHistory::new();
    assert!(history.is_empty());
    assert_eq!(history.len(), 0);

    history.push(user_msg("hello"));
    assert!(!history.is_empty());
    assert_eq!(history.len(), 1);
}

#[test]
fn test_as_slice() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push(assistant_msg("b"));
    let slice = history.as_slice();
    assert_eq!(slice.len(), 2);
}

#[test]
fn test_last_assistant_text_found() {
    let mut history = MessageHistory::new();
    history.push(user_msg("hello"));
    history.push(assistant_msg("first"));
    history.push(user_msg("more"));
    history.push(assistant_msg("second"));
    assert_eq!(history.last_assistant_text(), Some("second".to_string()));
}

#[test]
fn test_last_assistant_text_none() {
    let mut history = MessageHistory::new();
    history.push(user_msg("hello"));
    assert_eq!(history.last_assistant_text(), None);
}

#[test]
fn test_last_assistant_text_empty_content() {
    let mut history = MessageHistory::new();
    history.push(assistant_msg_empty());
    assert_eq!(history.last_assistant_text(), None);
}

#[test]
fn test_last_assistant_text_empty_history() {
    let history = MessageHistory::new();
    assert_eq!(history.last_assistant_text(), None);
}

#[test]
fn test_count_by_kind() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push(user_msg("b"));
    history.push(assistant_msg("c"));
    assert_eq!(history.count_by_kind(MessageKind::User), 2);
    assert_eq!(history.count_by_kind(MessageKind::Assistant), 1);
    assert_eq!(history.count_by_kind(MessageKind::System), 0);
}

#[test]
fn test_count_by_kind_empty() {
    let history = MessageHistory::new();
    assert_eq!(history.count_by_kind(MessageKind::User), 0);
}

#[test]
fn test_clear() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push(assistant_msg("b"));
    history.clear();
    assert!(history.is_empty());
    assert_eq!(history.len(), 0);
}

#[test]
fn test_truncate_keep_last_basic() {
    let mut history = MessageHistory::new();
    let msg_a = user_msg("a");
    let msg_b = assistant_msg("b");
    let msg_c = user_msg("c");
    let uuid_c = *msg_c.uuid().expect("uuid");
    history.push(msg_a);
    history.push(msg_b);
    history.push(msg_c);
    history.truncate_keep_last(2);
    assert_eq!(history.len(), 2);
    // The UUID index should still work for retained messages.
    assert!(history.find_by_uuid(&uuid_c).is_some());
}

#[test]
fn test_truncate_keep_last_n_larger() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.truncate_keep_last(10);
    assert_eq!(history.len(), 1);
}

#[test]
fn test_truncate_keep_last_zero() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push(user_msg("b"));
    history.truncate_keep_last(0);
    assert!(history.is_empty());
}

#[test]
fn test_truncate_keep_last_empty() {
    let mut history = MessageHistory::new();
    history.truncate_keep_last(5);
    assert!(history.is_empty());
}

#[test]
fn test_find_by_uuid() {
    let mut history = MessageHistory::new();
    let msg = user_msg("findme");
    let uuid = *msg.uuid().expect("uuid");
    history.push(msg);
    assert!(history.find_by_uuid(&uuid).is_some());
    assert!(history.find_by_uuid(&Uuid::new_v4()).is_none());
}

// ── LastUsageMarker invariants ───────────────────────────────────────

fn sample_usage(input: i64, output: i64) -> TokenUsage {
    TokenUsage {
        input_tokens: InputTokens {
            total: input,
            no_cache: input,
            cache_read: 0,
            cache_write: 0,
        },
        output_tokens: OutputTokens {
            total: output,
            text: output,
            reasoning: 0,
        },
    }
}

fn sample_model() -> ProviderModelSelection {
    ProviderModelSelection {
        provider: "anthropic".into(),
        model_id: "claude-opus-4-7".into(),
    }
}

#[test]
fn last_usage_is_none_initially() {
    let history = MessageHistory::new();
    assert!(history.last_usage().is_none());
    assert!(history.messages_since_last_usage().is_empty());
}

#[test]
fn push_assistant_with_usage_captures_anchor_at_current_length() {
    let mut history = MessageHistory::new();
    history.push(user_msg("hi"));
    history.push_assistant_with_usage(
        assistant_msg("hello"),
        sample_usage(1000, 200),
        sample_model(),
    );

    let marker = history.last_usage().expect("marker set");
    assert_eq!(marker.usage.input_tokens.total, 1000);
    assert_eq!(marker.usage.output_tokens.total, 200);
    // Tail is empty because anchor count = current len.
    assert!(history.messages_since_last_usage().is_empty());
}

#[test]
fn append_does_not_invalidate_marker() {
    let mut history = MessageHistory::new();
    history.push(user_msg("hi"));
    history.push_assistant_with_usage(
        assistant_msg("hello"),
        sample_usage(1000, 200),
        sample_model(),
    );

    // Append a tool_result-shaped message and a new user input.
    history.push(user_msg("tool result blob"));
    history.push(user_msg("follow-up"));

    assert!(
        history.last_usage().is_some(),
        "marker must survive appends"
    );
    assert_eq!(history.messages_since_last_usage().len(), 2);
}

#[test]
fn clear_invalidates_marker() {
    let mut history = MessageHistory::new();
    history.push_assistant_with_usage(assistant_msg("hi"), sample_usage(500, 100), sample_model());

    history.clear();
    assert!(history.last_usage().is_none());
}

#[test]
fn truncate_keep_last_invalidates_marker() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push_assistant_with_usage(assistant_msg("b"), sample_usage(500, 100), sample_model());
    history.push(user_msg("c"));

    history.truncate_keep_last(1);
    assert!(
        history.last_usage().is_none(),
        "truncate_keep_last must invalidate"
    );
}

#[test]
fn truncate_preserves_marker_when_anchor_within_keep_count() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    // anchor at count = 2 after the assistant push
    history.push_assistant_with_usage(assistant_msg("b"), sample_usage(500, 100), sample_model());
    history.push(user_msg("c"));
    history.push(user_msg("d"));

    // Truncate to keep 3 — anchor count (2) <= 3, marker stays valid.
    history.truncate(3);
    assert!(history.last_usage().is_some(), "anchor in retained range");
    assert_eq!(history.messages_since_last_usage().len(), 1);
}

#[test]
fn truncate_invalidates_marker_when_anchor_outside_keep_count() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push(assistant_msg("b"));
    history.push(user_msg("c"));
    // anchor at count = 4 after the assistant push
    history.push_assistant_with_usage(assistant_msg("d"), sample_usage(500, 100), sample_model());

    // Truncate to keep 2 — anchor count (4) > 2.
    history.truncate(2);
    assert!(history.last_usage().is_none());
}

#[test]
fn with_owned_messages_invalidates_marker() {
    let mut history = MessageHistory::new();
    history.push_assistant_with_usage(assistant_msg("hi"), sample_usage(500, 100), sample_model());

    // Even a no-op closure invalidates — body could have been rewritten.
    history.with_owned_messages(|_msgs| {});
    assert!(history.last_usage().is_none());
}

#[test]
fn messages_mut_invalidates_marker() {
    let mut history = MessageHistory::new();
    history.push_assistant_with_usage(assistant_msg("hi"), sample_usage(500, 100), sample_model());

    let _ = history.messages_mut();
    assert!(history.last_usage().is_none());
}

#[test]
fn drain_pushed_since_invalidates_marker() {
    let mut history = MessageHistory::new();
    history.push(user_msg("a"));
    history.push_assistant_with_usage(assistant_msg("b"), sample_usage(500, 100), sample_model());
    history.push(user_msg("c"));
    history.push(user_msg("d"));

    let _drained = history.drain_pushed_since(2);
    assert!(history.last_usage().is_none());
}

#[test]
fn re_anchoring_overwrites_previous_marker() {
    let mut history = MessageHistory::new();
    history.push_assistant_with_usage(
        assistant_msg("first"),
        sample_usage(100, 50),
        sample_model(),
    );

    history.push(user_msg("more"));
    history.push_assistant_with_usage(
        assistant_msg("second"),
        sample_usage(2000, 400),
        sample_model(),
    );

    let marker = history.last_usage().expect("marker");
    assert_eq!(marker.usage.input_tokens.total, 2000);
    assert!(history.messages_since_last_usage().is_empty());
}

#[test]
fn from_arcs_preserving_latest_usage_restores_tail_anchor() {
    let mut assistant = assistant_msg("metered");
    if let Message::Assistant(a) = &mut assistant {
        a.usage = Some(sample_usage(700, 80));
        a.model = "claude-haiku-4-5".into();
    }
    let messages = vec![
        Arc::new(user_msg("before")),
        Arc::new(assistant),
        Arc::new(user_msg("tail")),
    ];

    let history = MessageHistory::from_arcs_preserving_latest_usage(messages);

    let marker = history.last_usage().expect("marker restored");
    assert_eq!(marker.usage.total(), 780);
    assert_eq!(marker.model.model_id, "claude-haiku-4-5");
    assert_eq!(history.messages_since_last_usage().len(), 1);
}
