use super::*;

#[test]
fn test_json_parse_error_new() {
    let cause = serde_json::from_str::<serde_json::Value>("invalid json")
        .err()
        .unwrap();
    let error = JSONParseError::new("invalid json", Box::new(cause));
    assert_eq!(error.text, "invalid json");
    assert!(error.message.contains("JSON parsing failed"));
    assert!(error.cause.is_some());
}

#[test]
fn test_json_parse_error_display() {
    let cause = std::io::Error::other("parse failed");
    let error = JSONParseError::new("bad data", Box::new(cause));
    let display = format!("{error}");
    assert!(display.contains("JSON parsing failed"));
    assert!(display.contains("bad data"));
}
