use serde_json::json;

use super::*;

fn test_session_id(value: &str) -> crate::SessionId {
    match crate::SessionId::try_new(value) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id should be valid"),
    }
}

#[test]
fn remote_teammate_extras_session_id_wire_stays_string() {
    let extras = RemoteTeammateExtras {
        session_id: test_session_id("session-remote"),
        progress: None,
        error: None,
        result: None,
    };

    let value = serde_json::to_value(&extras).expect("serialize remote teammate extras");
    assert_eq!(value["session_id"], "session-remote");

    let decoded: RemoteTeammateExtras =
        serde_json::from_value(json!({ "session_id": "session-remote" }))
            .expect("deserialize remote teammate extras");
    assert_eq!(decoded.session_id.as_str(), "session-remote");
}
