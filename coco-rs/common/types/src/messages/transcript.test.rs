use serde_json::json;

use super::*;
use crate::MessageKind;
use crate::TombstoneMessage;

fn test_session_id(value: &str) -> crate::SessionId {
    match crate::SessionId::try_new(value) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id should be valid"),
    }
}

fn tombstone_message() -> Message {
    Message::Tombstone(TombstoneMessage {
        uuid: uuid::Uuid::nil(),
        original_kind: MessageKind::User,
    })
}

#[test]
fn transcript_message_session_id_wire_stays_string() {
    let value = TranscriptMessage {
        message: tombstone_message(),
        cwd: "/tmp/project".to_string(),
        user_type: "human".to_string(),
        session_id: test_session_id("session-1"),
        timestamp: "2026-07-07T00:00:00Z".to_string(),
        version: "1".to_string(),
        parent_uuid: None,
        logical_parent_uuid: None,
        is_sidechain: false,
        entrypoint: None,
        git_branch: None,
        agent_id: None,
        team_name: None,
        agent_name: None,
        agent_color: None,
        prompt_id: None,
    };

    let json = serde_json::to_value(&value).expect("TranscriptMessage serializes");
    assert_eq!(json["session_id"], "session-1");

    let decoded: TranscriptMessage =
        serde_json::from_value(json).expect("TranscriptMessage deserializes");
    assert_eq!(decoded.session_id.as_str(), "session-1");
}

#[test]
fn transcript_message_rejects_unsafe_session_id() {
    let value = json!({
        "message": {
            "type": "tombstone",
            "uuid": uuid::Uuid::nil(),
            "original_kind": "user"
        },
        "cwd": "/tmp/project",
        "user_type": "human",
        "session_id": "bad/session",
        "timestamp": "2026-07-07T00:00:00Z",
        "version": "1",
        "parent_uuid": null
    });

    let err = serde_json::from_value::<TranscriptMessage>(value)
        .expect_err("unsafe session id must be rejected");
    assert!(err.to_string().contains("path separator"), "got: {err}");
}
