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
fn serialized_message_session_id_wire_stays_string() {
    let value = SerializedMessage {
        message: tombstone_message(),
        cwd: "/tmp/project".to_string(),
        user_type: crate::UserType::Human,
        entrypoint: None,
        session_id: test_session_id("session-1"),
        timestamp: "2026-07-07T00:00:00Z".to_string(),
        version: "1".to_string(),
        git_branch: None,
        model_id: None,
    };

    let json = serde_json::to_value(&value).expect("SerializedMessage serializes");
    assert_eq!(json["session_id"], "session-1");

    let decoded: SerializedMessage =
        serde_json::from_value(json).expect("SerializedMessage deserializes");
    assert_eq!(decoded.session_id.as_str(), "session-1");
}

#[test]
fn serialized_message_rejects_unsafe_session_id() {
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
        "version": "1"
    });

    let err = serde_json::from_value::<SerializedMessage>(value)
        .expect_err("unsafe session id must be rejected");
    assert!(err.to_string().contains("path separator"), "got: {err}");
}
