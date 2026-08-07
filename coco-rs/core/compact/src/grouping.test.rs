use coco_messages::*;
use coco_types::*;
use uuid::Uuid;

use super::*;

fn user_msg() -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text("hello"),
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

fn assistant_msg() -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: "hi".into(),
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

fn assistant_msg_with_uuid(uuid: Uuid) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: "hi".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid,
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn tool_result_msg() -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(ToolResultContent {
                tool_call_id: "call_1".into(),
                tool_name: "Bash".into(),
                output: coco_llm_types::ToolResultContent::text("output"),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: "call_1".into(),
        tool_id: ToolId::Builtin(ToolName::Bash),
        is_error: false,
    })
}

#[test]
fn test_group_two_turns_by_assistant_id() {
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let msgs = vec![
        user_msg(),
        assistant_msg_with_uuid(id1),
        tool_result_msg(),
        assistant_msg_with_uuid(id2), // new assistant ID → new group
    ];
    let groups = group_messages_by_api_round(&msgs);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].len(), 3); // user + assistant(id1) + tool_result
    assert_eq!(groups[1].len(), 1); // assistant(id2)
}

#[test]
fn test_same_assistant_id_stays_in_one_group() {
    let id = Uuid::new_v4();
    let msgs = vec![
        user_msg(),
        assistant_msg_with_uuid(id),
        tool_result_msg(),
        // Same assistant ID (e.g., streaming chunks) → stays in same group
        assistant_msg_with_uuid(id),
    ];
    let groups = group_messages_by_api_round(&msgs);
    // All belong to one group since assistant ID doesn't change
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 4);
}

#[test]
fn test_agentic_session_multiple_rounds() {
    // Single user message, but multiple assistant rounds (agentic)
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let id3 = Uuid::new_v4();
    let msgs = vec![
        user_msg(),
        assistant_msg_with_uuid(id1),
        tool_result_msg(),
        assistant_msg_with_uuid(id2),
        tool_result_msg(),
        assistant_msg_with_uuid(id3),
    ];
    let groups = group_messages_by_api_round(&msgs);
    assert_eq!(groups.len(), 3, "should split on each new assistant ID");
}

#[test]
fn test_group_empty() {
    let groups = group_messages_by_api_round::<Message>(&[]);
    assert!(groups.is_empty());
}

#[test]
fn test_group_single_user() {
    let msgs = vec![user_msg()];
    let groups = group_messages_by_api_round(&msgs);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 1);
}

#[test]
fn test_no_assistant_all_in_one_group() {
    // Without assistant messages, everything lands in one group
    let msgs = vec![user_msg(), user_msg()];
    let groups = group_messages_by_api_round(&msgs);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
}

#[test]
fn test_classic_two_turn_conversation() {
    // Classic: user → assistant → user → assistant (all different UUIDs)
    let msgs = vec![user_msg(), assistant_msg(), user_msg(), assistant_msg()];
    let groups = group_messages_by_api_round(&msgs);
    assert_eq!(groups.len(), 2);
}
