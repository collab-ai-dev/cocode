use coco_types::*;
use uuid::Uuid;

use super::*;

#[test]
fn test_build_lookups_empty() {
    let lookups = build_message_lookups(&[]);
    assert!(lookups.message_by_uuid.is_empty());
}

#[test]
fn test_uuid_lookup() {
    let uuid = Uuid::new_v4();
    let msgs = vec![Message::Tombstone(TombstoneMessage {
        uuid,
        original_kind: MessageKind::User,
    })];
    let lookups = build_message_lookups(&msgs);
    assert_eq!(lookups.message_by_uuid.get(&uuid), Some(&0));
}

#[test]
fn test_tool_result_lookup() {
    let msgs = vec![
        crate::creation::create_user_message("hi"),
        Message::ToolResult(ToolResultMessage {
            uuid: Uuid::new_v4(),
            source_assistant_uuid: None,
            display_data: None,
            message: LlmMessage::Tool {
                content: vec![],
                provider_options: None,
            },
            tool_use_id: "call_123".into(),
            tool_id: ToolId::Builtin(ToolName::Read),
            is_error: false,
        }),
    ];
    let lookups = build_message_lookups(&msgs);
    assert_eq!(lookups.tool_result_ids.get("call_123"), Some(&1));
}
