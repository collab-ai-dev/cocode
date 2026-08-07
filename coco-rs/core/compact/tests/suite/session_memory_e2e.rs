//! Integration tests for session memory compaction (Type 3).
//!
//! Tests the no-LLM-call compaction path: use pre-extracted session memory
//! as the summary, with fallback to full compaction when memory is unavailable.

use std::sync::Arc;

use coco_compact::compact::CompactRunOptions;
use coco_compact::compact::compact_conversation;
use coco_compact::session_memory::SessionMemoryCompactConfig;
use coco_compact::session_memory::compact_session_memory;
use coco_messages::Message;
use coco_test_harness::compact as mock;
use coco_test_harness::conversation;
use coco_test_harness::messages as msg;
use coco_types::ToolName;

/// Test helper: wrap a borrowed `&[Message]` into the canonical
/// `Vec<Arc<Message>>` shape that the post-refactor compact entry points
/// accept. Caller keeps ownership of the source vec.
fn arc_vec(msgs: &[Message]) -> Vec<Arc<Message>> {
    msgs.iter().cloned().map(Arc::new).collect()
}

#[test]
fn test_basic_flow() {
    let messages = conversation::simple(5);
    let memory = "## Decisions\n- Chose Rust over Go\n- Using tokio for async\n\n## Context\n- Building CLI tool";
    let config = SessionMemoryCompactConfig::default();

    let result = compact_session_memory(&arc_vec(&messages), memory, None, &config)
        .expect("should not error")
        .expect("should produce a result");

    // Boundary should be in the dedicated field
    assert!(matches!(
        result.boundary_marker,
        Message::System(coco_messages::SystemMessage::CompactBoundary(_))
    ));

    // Summary should be a user message
    assert_eq!(result.summary_messages.len(), 1);
    assert!(matches!(result.summary_messages[0], Message::User(_)));
    if let Message::User(u) = &result.summary_messages[0] {
        assert!(u.is_compact_summary);
    }

    assert!(result.pre_compact_tokens > 0);
    assert!(!result.messages_to_keep.is_empty());
}

#[test]
fn test_api_invariant_preservation() {
    // Build conversation where the keep-index would land on a tool_result
    let mut messages = vec![
        msg::user("first request"),
        msg::assistant("first response"),
        msg::user("second request"),
        msg::assistant_with_tool_call("Bash", serde_json::json!({"command": "ls"})),
        msg::tool_result(ToolName::Bash, "call_Bash", &"x".repeat(500)),
        msg::assistant("final response"),
    ];
    // Add enough content to make the algorithm want to start mid-conversation
    for _ in 0..3 {
        messages.push(msg::user(&"padding ".repeat(100)));
        messages.push(msg::assistant(&"response ".repeat(100)));
    }

    let config = SessionMemoryCompactConfig {
        min_tokens: 100,
        min_text_block_messages: 1,
        max_tokens: 500,
        ..Default::default()
    };

    let result = compact_session_memory(&arc_vec(&messages), "Session context", None, &config)
        .expect("should not error")
        .expect("should produce a result");

    // Verify no tool_result is the first kept message (would break API invariants)
    if let Some(first) = result.messages_to_keep.first() {
        assert!(
            !matches!(first.as_ref(), Message::ToolResult(_)),
            "first kept message should not be a ToolResult (API invariant)"
        );
    }
}

#[test]
fn test_empty_returns_none() {
    let messages = conversation::simple(3);
    let config = SessionMemoryCompactConfig::default();

    assert!(
        compact_session_memory(&arc_vec(&messages), "", None, &config)
            .unwrap()
            .is_none()
    );
    assert!(
        compact_session_memory(&arc_vec(&messages), "   \n  ", None, &config)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_token_bounds() {
    // Very large conversation with large messages
    let messages = conversation::with_tool_results(ToolName::Bash, 20, 4000);
    let config = SessionMemoryCompactConfig {
        min_tokens: 5_000,
        min_text_block_messages: 2,
        max_tokens: 20_000,
        ..Default::default()
    };

    let result = compact_session_memory(&arc_vec(&messages), "Context summary", None, &config)
        .expect("should not error")
        .expect("should produce a result");

    // Should keep a subset, not all messages
    assert!(
        result.messages_to_keep.len() < messages.len(),
        "should compact some messages, kept {} of {}",
        result.messages_to_keep.len(),
        messages.len()
    );

    // Should keep at least some messages
    assert!(
        !result.messages_to_keep.is_empty(),
        "should keep at least some messages"
    );
}

// ── Fallback: session memory empty → full compaction ────────────────

#[tokio::test]
async fn test_session_memory_fallback_to_full_compact() {
    // This tests the real decision flow:
    // 1. Try session memory compaction → returns None (empty memory)
    // 2. Fall back to full LLM-based compaction → succeeds
    let messages = conversation::simple(5);
    let config = SessionMemoryCompactConfig::default();

    // Step 1: Session memory is empty → None
    let sm_result =
        compact_session_memory(&arc_vec(&messages), "", None, &config).expect("should not error");
    assert!(sm_result.is_none(), "empty memory should return None");

    // Step 2: Caller falls back to full compact
    let compact_run_options = CompactRunOptions {
        keep_recent_rounds: 1,
        ..Default::default()
    };
    let summary = "<analysis>x</analysis><summary>Fallback summary</summary>";
    let result = compact_conversation(
        &arc_vec(&messages),
        &compact_run_options,
        mock::mock_summarize_ok(summary),
        None,
    )
    .await
    .unwrap();

    mock::assert_boundary_valid(&result);
    mock::assert_summary_valid(&result);
}

#[tokio::test]
async fn test_session_memory_preferred_over_full() {
    // When session memory IS available, it should be used (no LLM call)
    let messages = conversation::simple(5);
    let memory = "## Decisions\n- Used Rust for performance\n## Context\n- Building CLI";
    let config = SessionMemoryCompactConfig::default();

    let sm_result = compact_session_memory(&arc_vec(&messages), memory, None, &config)
        .expect("should not error")
        .expect("session memory should produce result");

    // Verify it's a valid compaction without LLM call
    assert!(matches!(
        sm_result.boundary_marker,
        Message::System(coco_messages::SystemMessage::CompactBoundary(_))
    ));
    assert!(!sm_result.summary_messages.is_empty());

    // The summary should contain the session memory content
    if let Message::User(u) = &sm_result.summary_messages[0]
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
            text.contains("Used Rust for performance"),
            "summary should contain session memory content"
        );
    }
}
