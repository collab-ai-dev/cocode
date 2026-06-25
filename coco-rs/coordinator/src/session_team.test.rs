use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_session_team_name_uses_first_eight_chars() {
    assert_eq!(session_team_name("abcdef1234567890"), "session-abcdef12");
}

#[test]
fn test_session_team_name_short_id_is_safe() {
    assert_eq!(session_team_name("abc"), "session-abc");
    assert_eq!(session_team_name(""), "session-");
    assert_eq!(session_team_name("12345678"), "session-12345678");
}

#[test]
fn test_session_team_name_uuid_shape() {
    // Real session ids are UUIDs — only the first 8 hex chars are kept.
    assert_eq!(
        session_team_name("550e8400-e29b-41d4-a716-446655440000"),
        "session-550e8400"
    );
}
