//! Integration tests for full LLM-based compaction (Type 1).
//!
//! Tests compact_conversation with mock summarize_fn closures.
//! Also tests the combined flow: micro-compact → full compact.

use coco_compact::CompactSummaryKind;
use coco_compact::compact::CompactRunOptions;
use coco_compact::compact::compact_conversation;
use coco_compact::micro::micro_compact;
use coco_compact::types::CompactError;
use coco_messages::Message;
use coco_test_harness::compact as mock;
use coco_test_harness::conversation;
use coco_test_harness::messages as msg;
use coco_types::CompactTrigger;
use coco_types::ToolName;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const SUMMARY: &str =
    "<analysis>Analyzed the conversation.</analysis><summary>Key decisions made.</summary>";

/// Test helper: wrap a `Vec<Message>` into the canonical `Vec<Arc<Message>>`
/// shape that the post-refactor `compact_conversation` accepts.
fn arc_vec(msgs: &[Message]) -> Vec<Arc<Message>> {
    msgs.iter().cloned().map(Arc::new).collect()
}

#[tokio::test]
async fn test_basic_compact() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 2,
        ..Default::default()
    };

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
    assert_eq!(result.raw_summary.as_deref(), Some(SUMMARY));
    assert!(
        result.pre_compact_tokens > 0,
        "should estimate pre-compact tokens"
    );
    assert!(
        result.post_compact_tokens > 0,
        "should estimate post-compact tokens"
    );
    assert_eq!(result.trigger, CompactTrigger::Auto);
}

#[tokio::test]
async fn test_nothing_to_compact() {
    // Only 2 turns, keep_recent_rounds=2 → nothing to compact
    let messages = conversation::simple(2);
    let config = CompactRunOptions {
        keep_recent_rounds: 2,
        ..Default::default()
    };

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    assert!(
        result.summary_messages.is_empty(),
        "no summary when nothing to compact"
    );
    assert_eq!(result.messages_to_keep.len(), messages.len());
}

#[tokio::test]
async fn test_image_stripping() {
    let mut messages = conversation::simple(3);
    // Insert an image message
    messages.insert(2, msg::image_user());

    let (summarize, captured) = mock::mock_summarize_capturing(SUMMARY);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let _result = compact_conversation(&arc_vec(&messages), &config, summarize, None)
        .await
        .unwrap();

    let attempts = captured.lock().unwrap();
    assert!(!attempts.is_empty(), "summarize_fn should have been called");
    let attempt = &attempts[0];
    let rendered_messages = format!("{:?}", attempt.messages);
    let rendered_context_messages = format!("{:?}", attempt.context_messages);
    assert!(
        rendered_messages.contains("[image]"),
        "attempt messages should contain [image] placeholder"
    );
    assert!(
        rendered_context_messages.contains("[image]"),
        "attempt context messages should contain [image] placeholder"
    );
    assert!(
        !rendered_messages.contains("iVBORw0KGgo"),
        "attempt messages should NOT contain base64 image data"
    );
    assert!(
        !rendered_context_messages.contains("iVBORw0KGgo"),
        "attempt context messages should NOT contain base64 image data"
    );
}

#[tokio::test]
async fn test_full_compact_summary_attempt_is_structured() {
    let messages = conversation::simple(5);
    let (summarize, captured) = mock::mock_summarize_capturing(SUMMARY);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let _result = compact_conversation(&arc_vec(&messages), &config, summarize, None)
        .await
        .unwrap();

    let attempts = captured.lock().expect("capture lock poisoned");
    assert_eq!(attempts.len(), 1);
    let attempt = &attempts[0];
    assert_eq!(attempt.prompt_kind, CompactSummaryKind::Full);
    assert_eq!(
        serde_json::to_value(&attempt.context_messages).unwrap(),
        serde_json::to_value(&attempt.messages).unwrap()
    );
    assert_eq!(
        attempt.pre_compact_tokens,
        coco_messages::estimate_tokens_for_messages(&messages)
    );
    assert_eq!(attempt.max_summary_tokens, config.max_summary_tokens);
    assert!(
        !attempt
            .summary_request
            .contains("--- Conversation to summarize ---"),
        "conversation should stay in structured messages, not a legacy rendered prompt"
    );
    assert!(!attempt.summary_request.trim().is_empty());
    assert!(matches!(
        attempt.messages.first().map(std::sync::Arc::as_ref),
        Some(Message::User(_))
    ));
    assert!(
        attempt
            .messages
            .iter()
            .any(|message| matches!(message.as_ref(), Message::Assistant(_))),
        "structured attempt should preserve assistant-role messages"
    );
}

#[tokio::test]
async fn test_ptl_retry_updates_structured_attempt_messages() {
    let messages = conversation::simple(7);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };
    let captured = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let summarize = {
        let captured = captured.clone();
        let calls = calls.clone();
        move |attempt: coco_compact::CompactSummaryAttempt| {
            let captured = captured.clone();
            let calls = calls.clone();
            async move {
                captured
                    .lock()
                    .expect("capture lock poisoned")
                    .push(attempt);
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("prompt_too_long: input length exceeds context".to_string())
                } else {
                    Ok(coco_compact::CompactSummaryResponse {
                        summary: SUMMARY.to_string(),
                    })
                }
            }
        }
    };

    let result = compact_conversation(&arc_vec(&messages), &config, summarize, None)
        .await
        .expect("PTL retry should recover");

    mock::assert_summary_valid(&result);
    let attempts = captured.lock().expect("capture lock poisoned");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].prompt_kind, CompactSummaryKind::Full);
    assert_eq!(attempts[1].prompt_kind, CompactSummaryKind::Full);
    assert_eq!(attempts[0].summary_request, attempts[1].summary_request);
    assert!(
        attempts[1].messages.len() < attempts[0].messages.len(),
        "retry should drop the oldest API round from structured messages"
    );
    assert_eq!(
        serde_json::to_value(&attempts[1].context_messages).unwrap(),
        serde_json::to_value(&attempts[1].messages).unwrap(),
        "retry context should track the truncated structured messages"
    );
}

#[tokio::test]
async fn test_ptl_retry_succeeds() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    // First 2 calls fail with PTL, 3rd succeeds
    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ptl_then_ok(2, SUMMARY),
        None,
    )
    .await
    .unwrap();

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
}

#[tokio::test]
async fn test_ptl_exhausted() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let err = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_always_fail("prompt_too_long: exceeds context"),
        None,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, CompactError::PromptTooLong { .. }),
        "should be PromptTooLong, got: {err}"
    );
}

#[tokio::test]
async fn test_stream_retry() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    // First call fails with transient error, 2nd succeeds
    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_fail_then_ok(1, SUMMARY),
        None,
    )
    .await
    .unwrap();

    mock::assert_summary_valid(&result);
}

#[tokio::test]
async fn test_stream_exhausted() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let err = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_always_fail("network timeout"),
        None,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, CompactError::StreamRetryExhausted { .. }),
        "should be StreamRetryExhausted, got: {err}"
    );
}

#[tokio::test]
async fn test_empty_summary_error() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let err = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(""),
        None,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, CompactError::LlmCallFailed { .. }),
        "empty summary should be LlmCallFailed, got: {err}"
    );
}

#[tokio::test]
async fn test_attachment_callback() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let attachment_fn: coco_compact::compact::PostCompactAttachmentFn =
        Box::new(|_result: &coco_compact::CompactResult| {
            vec![coco_messages::AttachmentMessage::api(
                coco_types::AttachmentKind::CompactFileReference,
                coco_messages::LlmMessage::user_text("restored file content"),
            )]
        });

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        Some(attachment_fn),
    )
    .await
    .unwrap();

    assert_eq!(
        result.attachments.len(),
        1,
        "attachment callback should add 1 attachment"
    );
}

#[tokio::test]
async fn test_boundary_fields() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    let Message::System(coco_messages::SystemMessage::CompactBoundary(ref b)) =
        result.boundary_marker
    else {
        panic!("expected CompactBoundary");
    };
    assert!(b.tokens_before > 0);
    assert!(b.messages_summarized.is_some());
    assert!(b.messages_summarized.unwrap() > 0);
    assert_eq!(b.trigger, CompactTrigger::Auto);
}

#[tokio::test]
async fn test_custom_instructions() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        custom_prompt: Some("Focus on Rust refactoring decisions".to_string()),
        ..Default::default()
    };

    let (summarize, captured) = mock::mock_summarize_capturing(SUMMARY);
    let _result = compact_conversation(&arc_vec(&messages), &config, summarize, None)
        .await
        .unwrap();

    let attempts = captured.lock().unwrap();
    assert!(
        attempts[0]
            .summary_request
            .contains("Focus on Rust refactoring decisions")
    );
}

#[tokio::test]
async fn test_manual_trigger() {
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        trigger: CompactTrigger::Manual,
        ..Default::default()
    };

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.trigger, CompactTrigger::Manual);
    let Message::System(coco_messages::SystemMessage::CompactBoundary(ref b)) =
        result.boundary_marker
    else {
        panic!("expected CompactBoundary");
    };
    assert_eq!(b.trigger, CompactTrigger::Manual);
}

#[tokio::test]
async fn test_agentic_grouping() {
    // 1 user message + 5 assistant rounds (different UUIDs) → should group into 5 rounds
    // With keep_recent_rounds=2, should compact first 3 rounds
    let messages = conversation::agentic(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 2,
        ..Default::default()
    };

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
    // Recent 2 rounds = 2*(assistant+tool_result) = 4 messages kept
    assert!(
        result.messages_to_keep.len() <= 6,
        "should keep ~4-6 recent messages, got {}",
        result.messages_to_keep.len()
    );
}

// ── Combined flow: micro-compact as pre-step before full compact ────

#[tokio::test]
async fn test_micro_then_full_compact() {
    // This is the real-world flow when auto-compact triggers:
    // 1. Micro-compact: clear old tool results first (quick, no LLM)
    // 2. Full compact: summarize remaining conversation via LLM
    let mut messages = conversation::with_tool_results(ToolName::Bash, 8, 2000);

    // Stage 1: Micro-compact clears old tool results
    let micro_result = micro_compact(&mut messages, /*keep_recent*/ 4);
    assert!(
        micro_result.messages_cleared > 0,
        "micro should clear old tool results first"
    );

    // Stage 2: Full compact on the micro-compacted messages
    let config = CompactRunOptions {
        keep_recent_rounds: 2,
        ..Default::default()
    };
    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(SUMMARY),
        None,
    )
    .await
    .unwrap();

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
    assert!(
        !result.messages_to_keep.is_empty(),
        "should keep recent messages after full compact"
    );
}

#[tokio::test]
async fn test_full_compact_summary_format() {
    // Verify the summary is correctly formatted (analysis stripped, summary extracted)
    let messages = conversation::simple(5);
    let config = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };

    let raw_summary = "<analysis>Internal reasoning about the code changes.</analysis>\
                       <summary>\n1. Primary intent: refactor parser\n2. Files: src/parser.rs\n</summary>";

    let result = compact_conversation(
        &arc_vec(&messages),
        &config,
        mock::mock_summarize_ok(raw_summary),
        None,
    )
    .await
    .unwrap();

    // The summary message should have analysis stripped
    if let Message::User(u) = &result.summary_messages[0]
        && let coco_messages::LlmMessage::User { content, .. } = &u.message
    {
        let text: String = content
            .iter()
            .filter_map(|c| match c {
                coco_messages::UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !text.contains("<analysis>"),
            "analysis tags should be stripped from summary"
        );
        assert!(
            text.contains("refactor parser"),
            "summary content should be preserved"
        );
    }
}
