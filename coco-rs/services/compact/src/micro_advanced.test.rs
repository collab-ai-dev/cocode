use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::ToolResultMessage;
use uuid::Uuid;

use super::*;

fn make_tool_result(tool_use_id: &str, content: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![coco_messages::ToolContent::ToolResult(
                coco_messages::ToolResultContent {
                    tool_call_id: tool_use_id.to_string(),
                    tool_name: String::new(),
                    output: coco_llm_types::ToolResultContent::text(content.to_string()),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        tool_use_id: tool_use_id.to_string(),
        tool_id: coco_types::ToolId::Builtin(coco_types::ToolName::Read),
        is_error: false,
    })
}

fn make_assistant_with_thinking(text: &str, thinking: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![
                coco_messages::AssistantContent::Reasoning(coco_llm_types::ReasoningPart {
                    text: thinking.to_string(),
                    provider_metadata: None,
                }),
                coco_messages::AssistantContent::Text(coco_llm_types::TextPart::new(
                    text.to_string(),
                )),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn make_assistant_text(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![coco_messages::AssistantContent::Text(
                coco_llm_types::TextPart::new(text.to_string()),
            )],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn test_micro_compact_with_budget_clears_old_results() {
    let big_content = "x".repeat(2000); // ~500 tokens
    let mut messages = vec![
        make_tool_result("old_1", &big_content),
        make_tool_result("old_2", &big_content),
        make_tool_result("recent_1", "small result"),
    ];

    let config = MicroCompactBudgetConfig {
        tokens_to_free: 400,
        keep_recent: 1,
        exclude_tools: vec![],
        exclude_tool_strs: vec![],
    };

    let result = micro_compact_with_budget(&mut messages, &config);
    assert!(
        result.messages_cleared >= 1,
        "should clear at least one message"
    );
    assert!(result.tokens_saved_estimate > 0, "should save some tokens");

    // The recent message should be untouched
    let last = &messages[2];
    let content_str = format!("{last:?}");
    assert!(
        content_str.contains("small result"),
        "recent message should be preserved"
    );
}

#[test]
fn test_micro_compact_with_budget_respects_exclude() {
    let big_content = "x".repeat(2000);
    let mut messages = vec![make_tool_result("tool_1", &big_content)];

    let config = MicroCompactBudgetConfig {
        tokens_to_free: 1000,
        keep_recent: 0,
        exclude_tools: vec![coco_types::ToolName::Read],
        exclude_tool_strs: vec![],
    };

    let result = micro_compact_with_budget(&mut messages, &config);
    assert_eq!(
        result.messages_cleared, 0,
        "excluded tool should not be cleared"
    );
}

#[test]
fn test_clear_file_unchanged_stubs() {
    let mut messages = vec![
        make_tool_result("edit_1", "[file unchanged]"),
        make_tool_result("edit_2", "actual file content changed"),
        make_tool_result("edit_3", "[file unchanged]"),
    ];

    let result = clear_file_unchanged_stubs(&mut messages);
    assert_eq!(result.messages_cleared, 2, "should clear 2 unchanged stubs");

    // The middle message should be untouched
    let mid_str = format!("{:?}", messages[1]);
    assert!(
        mid_str.contains("actual file content"),
        "changed tool result should be preserved"
    );
}

#[test]
fn test_compact_thinking_blocks_removes_old_thinking() {
    let long_thinking = "Let me reason through this problem step by step. ".repeat(50);
    let mut messages = vec![
        make_assistant_with_thinking("Response 1", &long_thinking),
        make_assistant_with_thinking("Response 2", &long_thinking),
        make_assistant_with_thinking("Response 3 (recent)", &long_thinking),
    ];

    let result = compact_thinking_blocks(&mut messages, /*keep_recent_turns*/ 1);
    assert!(
        result.messages_cleared > 0,
        "should remove thinking blocks from old turns"
    );
    assert!(result.tokens_saved_estimate > 0, "should save tokens");

    // The most recent assistant message should still have thinking
    if let Message::Assistant(asst) = &messages[2]
        && let LlmMessage::Assistant { content, .. } = &asst.message
    {
        let has_thinking = content
            .iter()
            .any(|c| matches!(c, coco_messages::AssistantContent::Reasoning(_)));
        assert!(has_thinking, "recent turn should keep its thinking blocks");
    }

    // Old assistant messages should NOT have thinking
    if let Message::Assistant(asst) = &messages[0]
        && let LlmMessage::Assistant { content, .. } = &asst.message
    {
        let has_thinking = content
            .iter()
            .any(|c| matches!(c, coco_messages::AssistantContent::Reasoning(_)));
        assert!(
            !has_thinking,
            "old turn should have thinking blocks removed"
        );
    }
}

#[test]
fn test_compact_thinking_blocks_no_op_when_few_turns() {
    let mut messages = vec![make_assistant_text("Just text, no thinking")];

    let result = compact_thinking_blocks(&mut messages, /*keep_recent_turns*/ 5);
    assert_eq!(
        result.messages_cleared, 0,
        "nothing to compact with fewer turns than threshold"
    );
    assert_eq!(result.tokens_saved_estimate, 0);
}
