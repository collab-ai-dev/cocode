//! Integration tests for ALL micro-compaction variants:
//! - Basic micro-compact (tool result clearing with COMPACTABLE_TOOLS filter)
//! - Budget-aware micro-compact (stops at token target)
//! - API-level compact (clear_tool_uses, clear_thinking)
//! - Reactive api_microcompact (budget-targeted clearing on PTL)
//! - Combined pipeline (micro → thinking → api_compact in sequence)
//! - File-unchanged stub clearing
//! - Thinking block compaction

use coco_compact::micro::micro_compact;
use coco_compact::micro_advanced::MicroCompactBudgetConfig;
use coco_compact::micro_advanced::clear_file_unchanged_stubs;
use coco_compact::micro_advanced::compact_thinking_blocks;
use coco_compact::micro_advanced::micro_compact_with_budget;
use coco_compact::reactive;
use coco_compact::types::CLEARED_TOOL_RESULT_MESSAGE;
use coco_messages::Message;
use coco_test_harness::messages as msg;
use coco_types::ToolName;

#[test]
fn test_realistic_mixed_tools() {
    // Build a conversation with a mix of compactable and non-compactable tools.
    // Each tool_result is preceded by an assistant tool_use whose tool_call_id
    // matches — `collect_compactable_tool_ids` scans assistant messages for
    // compactable tool names.
    // Use distinct compactable tools so each generates a unique tool_use_id.
    let mut messages = vec![
        msg::user("do the work"),
        msg::assistant_with_tool_call("Read", serde_json::json!({"file_path": "/a"})),
        msg::tool_result(ToolName::Read, "call_Read", &"file content ".repeat(200)),
        msg::assistant_with_tool_call("Bash", serde_json::json!({"command": "ls"})),
        msg::tool_result(ToolName::Bash, "call_Bash", &"command output ".repeat(200)),
        msg::assistant_with_tool_call("Grep", serde_json::json!({"pattern": "x"})),
        msg::tool_result(ToolName::Grep, "call_Grep", &"grep results ".repeat(200)),
        // Non-compactable tools (Agent / TaskCreate / AskUserQuestion are
        // Builtin but NOT in COMPACTABLE_TOOLS, so they survive untouched
        // regardless of keep_recent).
        msg::tool_result(
            ToolName::Agent,
            "call_agent",
            &"agent response ".repeat(200),
        ),
        msg::tool_result(
            ToolName::TaskCreate,
            "call_task",
            &"task created ".repeat(200),
        ),
        msg::tool_result(
            ToolName::AskUserQuestion,
            "call_ask",
            &"user answer ".repeat(200),
        ),
        // Recent compactable: keep_recent=2 keeps {call_Bash, call_Grep, …}
        // — actually keeps the last 2 from a 4-id sequence so call_Glob is in
        // the keep set with call_Grep. call_Read (oldest) gets cleared.
        msg::assistant_with_tool_call("Glob", serde_json::json!({"pattern": "**/*.rs"})),
        msg::tool_result(ToolName::Glob, "call_Glob", &"recent glob ".repeat(200)),
    ];

    let result = micro_compact(&mut messages, /*keep_recent*/ 2);

    assert!(
        result.messages_cleared >= 2,
        "should clear compactable tool results"
    );

    // Verify non-compactable tools are untouched
    for m in &messages {
        if let Message::ToolResult(tr) = m
            && (tr.tool_use_id == "call_agent"
                || tr.tool_use_id == "call_task"
                || tr.tool_use_id == "call_ask")
        {
            let text = format!("{:?}", tr.message);
            assert!(
                !text.contains(CLEARED_TOOL_RESULT_MESSAGE),
                "non-compactable tool {id} should not be cleared",
                id = tr.tool_use_id
            );
        }
    }

    // Verify recent message is untouched
    if let Message::ToolResult(tr) = &messages[messages.len() - 1] {
        let text = format!("{:?}", tr.message);
        assert!(
            !text.contains(CLEARED_TOOL_RESULT_MESSAGE),
            "recent message should be preserved"
        );
    }
}

#[test]
fn test_idempotent() {
    let mut messages = vec![
        msg::user("test"),
        msg::assistant_with_tool_call_id("call_1", "Read", serde_json::json!({"file_path": "/a"})),
        msg::tool_result(ToolName::Read, "call_1", &"x".repeat(2000)),
        msg::assistant_with_tool_call_id("call_2", "Bash", serde_json::json!({"command": "ls"})),
        msg::tool_result(ToolName::Bash, "call_2", &"y".repeat(2000)),
        msg::assistant("done"),
    ];

    let first = micro_compact(&mut messages, /*keep_recent*/ 1);
    assert!(
        first.messages_cleared > 0,
        "first pass should clear something"
    );

    let second = micro_compact(&mut messages, /*keep_recent*/ 1);
    assert_eq!(
        second.messages_cleared, 0,
        "second pass should find nothing to clear (already-cleared skipped)"
    );
}

#[test]
fn test_budget_stops() {
    let mut messages: Vec<Message> = (0..10)
        .map(|i| msg::tool_result_large(ToolName::Read, &format!("call_{i}"), 2000))
        .collect();

    let config = MicroCompactBudgetConfig {
        tokens_to_free: 1200, // ~2.4 messages worth (~500 tokens each)
        keep_recent: 0,
        exclude_tools: vec![],
        exclude_tool_strs: vec![],
    };

    let result = micro_compact_with_budget(&mut messages, &config);
    // Should stop after clearing 2-3 messages (each ~500 tokens)
    assert!(
        result.messages_cleared >= 2 && result.messages_cleared <= 4,
        "should clear 2-4 messages to meet budget, got {}",
        result.messages_cleared
    );
    assert!(result.tokens_saved_estimate >= 1200);
}

#[test]
fn test_thinking_compact() {
    let thinking = "Let me think about this step by step. ".repeat(50);
    let mut messages = vec![
        msg::assistant_with_thinking("Response 1", &thinking),
        msg::assistant_with_thinking("Response 2", &thinking),
        msg::assistant_with_thinking("Response 3", &thinking),
        msg::assistant_with_thinking("Response 4", &thinking),
        msg::assistant_with_thinking("Response 5 (recent)", &thinking),
    ];

    let result = compact_thinking_blocks(&mut messages, /*keep_recent_turns*/ 2);

    assert!(result.messages_cleared > 0);

    // First 3 should have thinking removed
    for m in &messages[..3] {
        if let Message::Assistant(a) = m
            && let coco_messages::LlmMessage::Assistant { content, .. } = &a.message
        {
            let has_thinking = content
                .iter()
                .any(|c| matches!(c, coco_messages::AssistantContent::Reasoning(_)));
            assert!(!has_thinking, "old turn should have thinking removed");
        }
    }

    // Last 2 should keep thinking
    for m in &messages[3..] {
        if let Message::Assistant(a) = m
            && let coco_messages::LlmMessage::Assistant { content, .. } = &a.message
        {
            let has_thinking = content
                .iter()
                .any(|c| matches!(c, coco_messages::AssistantContent::Reasoning(_)));
            assert!(has_thinking, "recent turn should keep thinking");
        }
    }
}

#[test]
fn test_file_unchanged_stubs() {
    let mut messages = vec![
        msg::tool_result(ToolName::Edit, "edit_1", "[file unchanged]"),
        msg::tool_result(ToolName::Edit, "edit_2", "actual changes applied"),
        msg::tool_result(ToolName::Edit, "edit_3", "[file unchanged]"),
        msg::tool_result(ToolName::Write, "write_1", "file written successfully"),
    ];

    let result = clear_file_unchanged_stubs(&mut messages);
    assert_eq!(result.messages_cleared, 2, "should clear 2 unchanged stubs");

    // Verify actual changes are untouched
    if let Message::ToolResult(tr) = &messages[1] {
        let text = format!("{:?}", tr.message);
        assert!(
            text.contains("actual changes"),
            "real result should be preserved"
        );
    }
}

// ── API-level compact (clear_tool_uses + clear_thinking) ────────────

#[test]
fn test_api_clear_tool_uses_realistic() {
    // Build a realistic conversation with tool calls of varying sizes
    let big_input = format!(r#"{{"code": "{}"}}"#, "x".repeat(500));
    let mut messages = vec![
        msg::user("help me refactor"),
        msg::assistant_with_tool_call("Read", serde_json::json!({"file_path": "/src/main.rs"})),
        msg::tool_result(ToolName::Read, "call_Read", &"fn main() { }".repeat(50)),
        msg::assistant_with_tool_call("Bash", serde_json::from_str(&big_input).unwrap()),
        msg::tool_result(ToolName::Bash, "call_Bash", &"output ".repeat(100)),
        msg::assistant_with_tool_call(
            "Edit",
            serde_json::json!({"file_path": "/src/main.rs", "old_string": "x", "new_string": "y"}),
        ),
        msg::tool_result(ToolName::Edit, "call_Edit", "edit applied"),
        // Recent — should be kept
        msg::assistant_with_tool_call("Read", serde_json::json!({"file_path": "/src/lib.rs"})),
        msg::tool_result(ToolName::Read, "call_Read_2", "lib content"),
    ];

    let result = coco_compact::clear_tool_uses_inplace(
        &mut messages,
        /*keep_recent_count*/ 1,
        &[], // no excludes
    );

    assert!(
        result.messages_cleared >= 2,
        "should clear tool inputs from old messages"
    );
    assert!(result.tokens_saved_estimate > 0);

    // Recent assistant's tool call input should be preserved
    if let Message::Assistant(a) = &messages[messages.len() - 2]
        && let coco_messages::LlmMessage::Assistant { content, .. } = &a.message
    {
        for part in content {
            if let coco_messages::AssistantContent::ToolCall(tc) = part {
                assert_ne!(
                    tc.input,
                    serde_json::Value::Object(serde_json::Map::new()),
                    "recent tool call input should be preserved"
                );
            }
        }
    }
}

#[test]
fn test_api_clear_thinking_realistic() {
    let thinking = "Let me reason step by step about this refactoring. ".repeat(40);
    let mut messages = vec![
        msg::user("refactor the parser"),
        msg::assistant_with_thinking("I'll start with the lexer", &thinking),
        msg::assistant_with_thinking("Now the parser itself", &thinking),
        msg::assistant_with_thinking("Final cleanup", &thinking),
    ];

    let result = coco_compact::clear_thinking_inplace(&mut messages);

    assert_eq!(
        result.messages_cleared, 3,
        "should clear thinking from all 3 assistants"
    );
    assert!(result.tokens_saved_estimate > 0);

    // All assistant messages should still have text content
    for m in &messages {
        if let Message::Assistant(a) = m
            && let coco_messages::LlmMessage::Assistant { content, .. } = &a.message
        {
            let has_text = content
                .iter()
                .any(|c| matches!(c, coco_messages::AssistantContent::Text(_)));
            assert!(
                has_text,
                "text content should be preserved after clearing thinking"
            );
        }
    }
}

// ── Reactive api_microcompact ───────────────────────────────────────

#[test]
fn test_reactive_api_microcompact() {
    // Simulate what happens when the query engine gets a prompt_too_long error:
    // it calls calculate_drop_target then api_microcompact
    let mut messages = vec![
        msg::user("implement the feature"),
        msg::tool_result(ToolName::Bash, "call_0", &"x".repeat(4000)),
        msg::tool_result(ToolName::Read, "call_1", &"y".repeat(4000)),
        msg::tool_result(ToolName::Grep, "call_2", &"z".repeat(4000)),
        msg::tool_result(ToolName::Read, "call_3", &"w".repeat(4000)),
        msg::assistant("working on it"),
    ];

    let config = reactive::ReactiveCompactConfig {
        context_window: 5_000,
        max_output_tokens: 1_000,
        ..Default::default()
    };

    let current_tokens = coco_messages::estimate_tokens_for_messages(&messages);
    let drop_target = reactive::calculate_drop_target(
        current_tokens,
        &config,
        &coco_config::AutoCompactConfig::default(),
    );
    assert!(drop_target > 0, "should need to drop tokens");

    reactive::api_microcompact(&mut messages, drop_target);

    // Some tool results should be cleared
    let cleared_count = messages
        .iter()
        .filter(|m| {
            if let Message::ToolResult(tr) = m {
                format!("{:?}", tr.message).contains(CLEARED_TOOL_RESULT_MESSAGE)
            } else {
                false
            }
        })
        .count();
    assert!(cleared_count > 0, "reactive should clear some tool results");
}

// ── Combined micro pipeline ─────────────────────────────────────────

#[test]
fn test_combined_micro_pipeline() {
    // Real-world flow: micro_compact → compact_thinking → clear_tool_uses
    // Each stage handles different content, no overlap.
    let thinking = "Reasoning about the problem. ".repeat(30);
    let big_input = format!(r#"{{"path": "{}"}}"#, "a".repeat(400));

    let mut messages = vec![
        msg::user("do everything"),
        // Old tool results (micro_compact target)
        msg::assistant_with_tool_call_id("call_0", "Read", serde_json::json!({"file_path": "a"})),
        msg::tool_result(ToolName::Read, "call_0", &"old file content ".repeat(200)),
        msg::assistant_with_tool_call_id("call_1", "Bash", serde_json::json!({"command": "cat a"})),
        msg::tool_result(ToolName::Bash, "call_1", &"old bash output ".repeat(200)),
        // Old thinking blocks (compact_thinking target)
        msg::assistant_with_thinking("Old analysis", &thinking),
        // Old tool call inputs (clear_tool_uses target)
        msg::assistant_with_tool_call("Bash", serde_json::from_str(&big_input).unwrap()),
        msg::tool_result(ToolName::Bash, "call_Bash", "recent output"),
        // Recent — untouched by all
        msg::assistant_with_thinking("Recent work", &thinking),
        msg::assistant("final answer"),
    ];

    let total_before = messages.len();

    // Stage 1: Clear old tool results
    let micro_result = micro_compact(&mut messages, /*keep_recent*/ 1);
    assert!(
        micro_result.messages_cleared > 0,
        "micro should clear tool results"
    );

    // Stage 2: Clear old thinking blocks
    let thinking_result = compact_thinking_blocks(&mut messages, /*keep_recent*/ 1);
    assert!(
        thinking_result.messages_cleared > 0,
        "should clear old thinking"
    );

    // Stage 3: Clear old tool call inputs
    let api_result =
        coco_compact::clear_tool_uses_inplace(&mut messages, /*keep_recent*/ 1, &[]);

    // Total tokens freed should be substantial
    let total_freed = micro_result.tokens_saved_estimate
        + thinking_result.tokens_saved_estimate
        + api_result.tokens_saved_estimate;
    assert!(
        total_freed > 500,
        "combined pipeline should free significant tokens"
    );

    // Message count unchanged (micro no longer tombstones)
    assert_eq!(messages.len(), total_before);
}
