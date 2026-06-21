use super::*;
use serde_json::json;

#[test]
fn test_parse_partial_json_complete() {
    let json = r#"{"name": "test", "value": 42}"#;
    let result = parse_partial_json(json);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), json!({"name": "test", "value": 42}));
}

#[test]
fn test_parse_partial_json_incomplete_object() {
    let json = r#"{"name": "test""#;
    let result = parse_partial_json(json);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), json!({"name": "test"}));
}

#[test]
fn test_parse_partial_json_incomplete_array() {
    let json = r#"[1, 2, 3"#;
    let result = parse_partial_json(json);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), json!([1, 2, 3]));
}

#[test]
fn test_parse_partial_json_nested() {
    let json = r#"{"outer": {"inner": "value""#;
    let result = parse_partial_json(json);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), json!({"outer": {"inner": "value"}}));
}

#[test]
fn test_complete_partial_json() {
    let incomplete = r#"{"a": [1, 2"#;
    let completed = complete_partial_json(incomplete);
    assert_eq!(completed, r#"{"a": [1, 2]}"#);
}

#[test]
fn test_complete_partial_json_string() {
    let incomplete = r#"{"text": "hello"#;
    let completed = complete_partial_json(incomplete);
    assert_eq!(completed, r#"{"text": "hello"}"#);
}

#[test]
fn test_parse_partial_json_with_trailing_comma() {
    let json = r#"{"items": [1, 2, 3,], "name": "test",}"#;
    let result = parse_partial_json_with_repair(json);
    assert!(result.is_some());
}

#[test]
fn test_extract_partial_value() {
    let json = r#"{"user": {"name": "Alice", "age": 30}"#;
    let result = extract_partial_value(json, &["user", "name"]);
    assert_eq!(result, Some(json!("Alice")));
}

#[test]
fn test_extract_partial_value_array() {
    let json = r#"{"items": ["a", "b", "c"]}"#;
    let result = extract_partial_value(json, &["items", "1"]);
    assert_eq!(result, Some(json!("b")));
}

#[test]
fn test_parse_partial_json_invalid() {
    let invalid = r#"not json at all"#;
    let result = parse_partial_json(invalid);
    assert!(result.is_none());
}
