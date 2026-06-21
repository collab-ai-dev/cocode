//! Tests for repair_text.rs

use super::*;
use serde_json::Value;

#[test]
fn test_repair_valid_json() {
    let json = r#"{"name": "test"}"#;
    let result = repair_json_text(json);
    assert!(result.is_some());
}

#[test]
fn test_repair_truncated_json() {
    let json = r#"{"name": "test""#;
    let result = repair_json_text(json);
    assert!(result.is_some());
    let parsed: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(parsed["name"], "test");
}

#[test]
fn test_repair_trailing_comma() {
    let json = r#"{"items": [1, 2, 3, ]}"#;
    let result = repair_json_text(json);
    assert!(result.is_some());
}
