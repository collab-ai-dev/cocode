//! Integration tests for `partial_compact_conversation` and the
//! token-gap-aware PTL retry path, plus the reactive `peel_head` helper.

use coco_compact::CompactSummaryKind;
use coco_compact::CompactSummaryResponse;
use coco_compact::compact::partial_compact_conversation;
use coco_compact::compact::truncate_head_for_ptl_retry;
use coco_compact::reactive::peel_head_for_ptl_retry;
use coco_messages::Message;
use coco_messages::PartialCompactDirection;
use coco_messages::SystemMessage;
use coco_test_harness::compact as mock;
use coco_test_harness::conversation;
use coco_test_harness::messages as msg;
use coco_types::CompactTrigger;
use std::future::Future;
use std::pin::Pin;

const SUMMARY: &str = "<analysis>Reviewed.</analysis><summary>Summary of recent work.</summary>";

/// Test helper: wrap a borrowed `&[Message]` into the canonical
/// `Vec<Arc<Message>>` shape that the post-refactor compact entry points
/// accept. Caller keeps ownership of the source vec.
fn arc_vec(msgs: &[Message]) -> Vec<std::sync::Arc<Message>> {
    msgs.iter().cloned().map(std::sync::Arc::new).collect()
}

#[tokio::test]
async fn partial_newest_summarizes_tail_keeps_prefix() {
    // 6 turns; pivot at index 4 → summarize messages[4..], keep messages[..4].
    let messages = conversation::simple(6);
    let (summarize, captured) = mock::mock_summarize_capturing(SUMMARY);
    let result = partial_compact_conversation(
        &arc_vec(&messages),
        4,
        PartialCompactDirection::Newest,
        None,
        None,
        summarize,
        None,
    )
    .await
    .expect("partial compact should succeed");

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
    assert_eq!(result.raw_summary.as_deref(), Some(SUMMARY));
    assert_eq!(result.trigger, CompactTrigger::Manual);

    // For Newest direction, prefix is kept.
    assert!(!result.messages_to_keep.is_empty());

    // Boundary must record the preserved segment with the boundary uuid as anchor.
    let Message::System(SystemMessage::CompactBoundary(b)) = &result.boundary_marker else {
        panic!("expected CompactBoundary");
    };
    let seg = b
        .preserved_segment
        .as_ref()
        .expect("preserved_segment should be set");
    assert_eq!(seg.anchor_uuid, b.uuid, "Newest anchor must be boundary");

    let attempts = captured.lock().expect("capture lock poisoned");
    assert_eq!(attempts.len(), 1);
    let attempt = &attempts[0];
    assert_eq!(attempt.prompt_kind, CompactSummaryKind::Partial);
    assert_eq!(attempt.messages.len(), messages.len() - 4);
    assert_eq!(
        attempt.context_messages.len(),
        messages.len(),
        "Newest/from partial compact should keep full structured context on the first attempt"
    );
    assert_eq!(
        attempt.max_summary_tokens,
        coco_compact::types::MAX_OUTPUT_TOKENS_FOR_SUMMARY
    );
    assert!(
        !attempt
            .summary_request
            .contains("--- Conversation to summarize ---"),
        "partial compact should also keep conversation in structured messages"
    );
}

#[tokio::test]
async fn partial_oldest_summarizes_prefix_keeps_tail() {
    let messages = conversation::simple(6);
    let result = partial_compact_conversation(
        &arc_vec(&messages),
        2,
        PartialCompactDirection::Oldest,
        Some("focus on tests"),
        None,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .expect("partial compact should succeed");

    let Message::System(SystemMessage::CompactBoundary(b)) = &result.boundary_marker else {
        panic!("expected CompactBoundary");
    };
    // user_feedback was supplied — must propagate to user_context.
    assert_eq!(b.user_context.as_deref(), Some("focus on tests"));

    // Anchor for Oldest = last summary uuid (suffix-preserving chain).
    let summary_uuid = match &result.summary_messages[0] {
        Message::User(u) => u.uuid,
        _ => panic!("summary[0] must be User"),
    };
    let seg = b
        .preserved_segment
        .as_ref()
        .expect("preserved_segment should be set");
    assert_eq!(
        seg.anchor_uuid, summary_uuid,
        "Oldest anchor = last summary"
    );
}

#[tokio::test]
async fn partial_empty_summarize_errors() {
    let messages = conversation::simple(2);
    // Newest at end → nothing to summarize.
    let res = partial_compact_conversation(
        &arc_vec(&messages),
        messages.len(),
        PartialCompactDirection::Newest,
        None,
        None,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await;
    assert!(res.is_err(), "should error when summarize range is empty");
}

#[test]
fn truncate_head_uses_token_gap_when_provided() {
    let messages = conversation::simple(4);
    // Provide a tiny gap → should drop just one group.
    let truncated = truncate_head_for_ptl_retry(&arc_vec(&messages), Some(1), 0.2)
        .expect("with multiple groups, returns a survivor list");
    assert!(
        truncated.len() < messages.len(),
        "should drop at least one group"
    );
}

#[test]
fn truncate_head_strips_stale_marker_before_grouping() {
    use coco_messages::LlmMessage;
    use coco_messages::UserMessage;
    use uuid::Uuid;

    // Marker at the head + 4 turns → grouping must skip the marker first.
    let mut messages = vec![Message::User(UserMessage {
        message: LlmMessage::user_text("[earlier conversation truncated for compaction retry]"),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: true,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })];
    messages.extend(conversation::simple(4));

    // Without the strip, the function would see only the marker as group 0
    // and might fail. With the strip, it operates on the real conversation.
    let result = truncate_head_for_ptl_retry(&arc_vec(&messages), None, 0.5);
    assert!(
        result.is_some(),
        "PTL retry should succeed when stripping stale marker"
    );
}

#[test]
fn truncate_head_returns_none_with_one_group() {
    // Single user message, no assistant ⇒ one group ⇒ nothing to drop.
    let messages = vec![msg::user("only one message")];
    assert!(truncate_head_for_ptl_retry(&arc_vec(&messages), None, 0.5).is_none());
}

#[test]
fn peel_head_drops_oldest_groups() {
    // Build a multi-round conversation by alternating user/assistant.
    let messages: Vec<std::sync::Arc<Message>> = conversation::simple(6)
        .into_iter()
        .map(std::sync::Arc::new)
        .collect();
    let total_tokens = coco_messages::estimate_tokens_for_messages(&messages);
    let target = total_tokens / 2;
    let peeled = peel_head_for_ptl_retry(&messages, target).expect("should peel some groups");
    assert!(
        peeled.len() < messages.len(),
        "must drop at least one group"
    );
    // After peeling, total tokens are lower.
    assert!(coco_messages::estimate_tokens_for_messages(&peeled) <= total_tokens);
}

#[test]
fn peel_head_returns_none_for_single_group() {
    let messages: Vec<std::sync::Arc<Message>> = vec![std::sync::Arc::new(msg::user("hi"))];
    assert!(peel_head_for_ptl_retry(&messages, 1).is_none());
}

#[test]
fn build_post_compact_messages_has_canonical_order() {
    let mut result = mock::dummy_compact_result();
    result.summary_messages.push(msg::user("summary"));
    result
        .messages_to_keep
        .push(std::sync::Arc::new(msg::user("kept")));
    result.hook_results.push(msg::user("hook"));

    let assembled = coco_compact::build_post_compact_messages(&result);
    assert_eq!(assembled.len(), 4); // boundary + summary + kept + hook
    matches!(
        &*assembled[0],
        Message::System(SystemMessage::CompactBoundary(_))
    );
}

#[test]
fn build_partial_post_compact_messages_newest_keeps_prefix_before_summary() {
    let mut result = mock::dummy_compact_result();
    result.summary_messages.push(msg::user("summary"));
    result
        .messages_to_keep
        .push(std::sync::Arc::new(msg::user("kept prefix")));
    result.hook_results.push(msg::user("hook"));

    let assembled =
        coco_compact::build_partial_post_compact_messages(&result, PartialCompactDirection::Newest);

    assert_eq!(assembled.len(), 4);
    matches!(
        &*assembled[0],
        Message::System(SystemMessage::CompactBoundary(_))
    );
    assert_eq!(
        coco_compact::summary_text::extract_message_text(&assembled[1]).as_deref(),
        Some("kept prefix")
    );
    assert_eq!(
        coco_compact::summary_text::extract_message_text(&assembled[2]).as_deref(),
        Some("summary")
    );
    assert_eq!(
        coco_compact::summary_text::extract_message_text(&assembled[3]).as_deref(),
        Some("hook")
    );
}

#[tokio::test]
async fn partial_newest_ptl_retry_truncates_full_context_not_tail_only() {
    let messages = conversation::simple(12);
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let summarize = {
        let captured = captured.clone();
        let calls = calls.clone();
        move |attempt: coco_compact::CompactSummaryAttempt| {
            let captured = captured.clone();
            let calls = calls.clone();
            Box::pin(async move {
                captured
                    .lock()
                    .expect("capture lock poisoned")
                    .push(attempt);
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err("prompt_too_long: input exceeds context".to_string())
                } else {
                    Ok(CompactSummaryResponse {
                        summary: SUMMARY.to_string(),
                    })
                }
            })
                as Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
        }
    };

    partial_compact_conversation(
        &arc_vec(&messages),
        8,
        PartialCompactDirection::Newest,
        None,
        None,
        summarize,
        None,
    )
    .await
    .expect("partial compact should recover from one PTL retry");

    let attempts = captured.lock().expect("capture lock poisoned");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].messages.len(), messages.len() - 8);
    assert_eq!(attempts[0].context_messages.len(), messages.len());
    assert!(
        attempts[1].context_messages.len() < attempts[0].context_messages.len(),
        "retry should truncate the full API context"
    );
    assert_eq!(
        attempts[1].messages.len(),
        attempts[0].messages.len(),
        "retry should keep the selected tail summary slice while the preserved prefix absorbs the drop"
    );
    assert!(
        attempts[1].context_messages.len() > attempts[1].messages.len(),
        "retry context should still include preserved prefix messages"
    );
}

#[test]
fn merge_hook_instructions_combines_both() {
    let merged = coco_compact::merge_hook_instructions(Some("user text"), Some("hook addition"));
    assert_eq!(merged.as_deref(), Some("user text\n\nhook addition"));

    assert_eq!(
        coco_compact::merge_hook_instructions(Some("only user"), None).as_deref(),
        Some("only user")
    );
    assert_eq!(
        coco_compact::merge_hook_instructions(None, Some("only hook")).as_deref(),
        Some("only hook")
    );
    assert_eq!(coco_compact::merge_hook_instructions(None, None), None);
    // Empty strings collapse to None.
    assert_eq!(
        coco_compact::merge_hook_instructions(Some("   "), Some("   ")),
        None
    );
}

#[test]
fn extract_discovered_tool_names_picks_up_toolsearch_input() {
    use coco_messages::AssistantContent;
    use coco_messages::AssistantMessage;
    use coco_messages::LlmMessage;
    use serde_json::json;
    use uuid::Uuid;

    let messages = vec![Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(coco_llm_types::ToolCallPart {
                tool_call_id: "call_ts".into(),
                tool_name: "ToolSearch".into(),
                input: json!({"tools": ["Bash", "Edit"]}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })];

    let names = coco_compact::extract_discovered_tool_names(&messages);
    assert_eq!(names.len(), 2);
    assert!(names.contains("Bash"));
    assert!(names.contains("Edit"));
}

/// Images nested inside `tool_result.content` arrays must be replaced with
/// `[image]` placeholders before compact summary, not just images at the top
/// level of user messages. Without this, BashTool tool_results containing
/// detected image bytes (`is_likely_image_bytes` → `structuredContent`)
/// survive the strip and re-trip prompt-too-long during compaction.
#[test]
fn strip_images_walks_tool_result_content() {
    use coco_llm_types::ToolResultContent;
    use coco_llm_types::ToolResultContentPart;
    use coco_messages::LlmMessage;
    use coco_messages::ToolContent;
    use coco_messages::ToolResultContent as InternalTrc;
    use coco_messages::ToolResultMessage;
    use coco_types::ToolId;
    use coco_types::ToolName;
    use uuid::Uuid;

    let image_part = ToolResultContentPart::FileData {
        data: "iVBORw0KGgoBigBase64Payload==".to_string(),
        media_type: "image/png".to_string(),
        filename: Some("out.png".to_string()),
        provider_options: None,
    };
    let tool_result = Message::ToolResult(ToolResultMessage {
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(InternalTrc {
                tool_call_id: "abc".into(),
                tool_name: "Bash".into(),
                output: ToolResultContent::Content {
                    value: vec![image_part],
                    provider_options: None,
                },
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        tool_use_id: "abc".into(),
        tool_id: ToolId::Builtin(ToolName::Bash),
        is_error: false,
    });

    let stripped = coco_compact::strip_images_from_messages(&[tool_result]);
    assert_eq!(stripped.len(), 1);
    let Message::ToolResult(tr) = &stripped[0] else {
        panic!("expected tool_result");
    };
    let LlmMessage::Tool { content, .. } = &tr.message else {
        panic!("expected tool LlmMessage");
    };
    let ToolContent::ToolResult(rp) = &content[0] else {
        panic!("expected tool_result part");
    };
    let ToolResultContent::Content { value, .. } = &rp.output else {
        panic!("expected Content variant");
    };
    assert_eq!(value.len(), 1);
    let ToolResultContentPart::Text { text, .. } = &value[0] else {
        panic!("FileData should have been replaced with Text");
    };
    assert_eq!(text, "[image]");
    assert!(
        !format!("{value:?}").contains("iVBORw0KGgo"),
        "raw base64 image data must not survive strip_images"
    );
}
