use crate::types::CLEARED_TOOL_RESULT_MESSAGE;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::ToolCallPart;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::ToolResultContent;
use coco_messages::ToolResultMessage;
use coco_types::StopReason;
use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::*;

fn make_assistant_tool_call(tool_call_id: &str, tool_name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![AssistantContentPart::ToolCall(ToolCallPart::new(
            tool_call_id,
            tool_name,
            serde_json::json!({}),
        ))]),
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn make_tool_result(tool_call_id: &str, tool_id: ToolId, text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(ToolResultContent {
                tool_call_id: tool_call_id.to_string(),
                tool_name: String::new(),
                output: coco_llm_types::ToolResultContent::text(text.to_string()),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: tool_call_id.to_string(),
        tool_id,
        is_error: false,
    })
}

#[test]
fn test_micro_compact_floors_keep_recent_to_one_and_clears_short_old_result() {
    let mut messages = vec![
        make_assistant_tool_call("old_read", ToolName::Read.as_str()),
        make_tool_result(
            "old_read",
            ToolId::Builtin(ToolName::Read),
            "short old result",
        ),
        make_assistant_tool_call("recent_read", ToolName::Read.as_str()),
        make_tool_result(
            "recent_read",
            ToolId::Builtin(ToolName::Read),
            "short recent result",
        ),
    ];

    let result = micro_compact(&mut messages, 0);

    assert_eq!(result.messages_cleared, 1);
    assert!(format!("{:?}", messages[1]).contains(CLEARED_TOOL_RESULT_MESSAGE));
    assert!(format!("{:?}", messages[3]).contains("short recent result"));
}

#[test]
fn test_micro_compact_preserves_recovery_pointers() {
    // A windowed inline result (suffix `</persisted-output>` footer) is
    // REDUCED to its recovery footer — bulk freed, pointer survives. A full
    // persisted reference (prefix tag) is already minimal and stays intact.
    // Plain results clear to the bare placeholder.
    let mut messages = vec![
        make_assistant_tool_call("old_plain", ToolName::Bash.as_str()),
        make_tool_result("old_plain", ToolId::Builtin(ToolName::Bash), "plain output"),
        make_assistant_tool_call("old_windowed", ToolName::Bash.as_str()),
        make_tool_result(
            "old_windowed",
            ToolId::Builtin(ToolName::Bash),
            "big head text\n\n[... middle omitted ...]\n\ntail\n\n<persisted-output>\nFull text saved to: /s/x.txt\n</persisted-output>",
        ),
        make_assistant_tool_call("old_reference", ToolName::Bash.as_str()),
        make_tool_result(
            "old_reference",
            ToolId::Builtin(ToolName::Bash),
            "<persisted-output>\nFull output saved to: /s/y.txt\n</persisted-output>",
        ),
        make_assistant_tool_call("recent", ToolName::Bash.as_str()),
        make_tool_result("recent", ToolId::Builtin(ToolName::Bash), "recent output"),
    ];

    let result = micro_compact(&mut messages, 1);

    // Plain cleared + windowed reduced = 2; the minimal reference is skipped.
    assert_eq!(result.messages_cleared, 2);
    assert!(format!("{:?}", messages[1]).contains(CLEARED_TOOL_RESULT_MESSAGE));
    // Windowed: body gone, recovery pointer retained.
    let windowed = format!("{:?}", messages[3]);
    assert!(windowed.contains(CLEARED_TOOL_RESULT_MESSAGE));
    assert!(windowed.contains("/s/x.txt"));
    assert!(!windowed.contains("big head text"));
    // Prefix reference: intact.
    let reference = format!("{:?}", messages[5]);
    assert!(!reference.contains(CLEARED_TOOL_RESULT_MESSAGE));
    assert!(reference.contains("/s/y.txt"));
    assert!(format!("{:?}", messages[7]).contains("recent output"));

    // Idempotency: a second pass leaves everything byte-identical.
    let before = format!("{messages:?}");
    let second = micro_compact(&mut messages, 1);
    assert_eq!(second.messages_cleared, 0);
    assert_eq!(format!("{messages:?}"), before);
}

#[test]
fn test_micro_compact_ignores_non_compactable_custom_tools() {
    let mut messages = vec![
        make_assistant_tool_call("custom_1", "CustomTool"),
        make_tool_result(
            "custom_1",
            ToolId::Custom("CustomTool".to_string()),
            "custom output that should not be microcompacted",
        ),
        make_assistant_tool_call("read_1", ToolName::Read.as_str()),
        make_tool_result(
            "read_1",
            ToolId::Builtin(ToolName::Read),
            "recent read output",
        ),
    ];

    let result = micro_compact(&mut messages, 1);

    assert_eq!(result.messages_cleared, 0);
    assert!(format!("{:?}", messages[1]).contains("custom output"));
}
