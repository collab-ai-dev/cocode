use super::*;
use coco_types::SessionId;
use pretty_assertions::assert_eq;

#[test]
fn test_session_team_name_uses_first_eight_chars() {
    let session_id = SessionId::try_new("abcdef1234567890").unwrap();
    assert_eq!(session_team_name(&session_id), "session-abcdef12");
}

#[test]
fn test_session_team_name_short_id_is_safe() {
    let short = SessionId::try_new("abc").unwrap();
    let exact = SessionId::try_new("12345678").unwrap();
    assert_eq!(session_team_name(&short), "session-abc");
    assert_eq!(session_team_name(&exact), "session-12345678");
}

#[test]
fn test_session_team_name_uuid_shape() {
    // Real session ids are UUIDs — only the first 8 hex chars are kept.
    let session_id = SessionId::try_new("550e8400-e29b-41d4-a716-446655440000").unwrap();
    assert_eq!(session_team_name(&session_id), "session-550e8400");
}
