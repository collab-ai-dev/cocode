use super::*;

#[test]
fn test_parse_commit_sha() {
    assert_eq!(parse_commit_sha("abc1234"), Some("abc1234".to_string()));
    assert_eq!(
        parse_commit_sha("[main abc1234] msg"),
        Some("abc1234".to_string())
    );
    assert_eq!(parse_commit_sha("no sha here"), None);
    assert_eq!(parse_commit_sha("short a1"), None); // too short
}
