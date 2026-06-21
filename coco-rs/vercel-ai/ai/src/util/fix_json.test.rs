//! Tests for fix_json.rs

use super::*;

#[test]
fn test_fix_valid_json() {
    let json = r#"{"name": "test"}"#;
    let result = fix_json(json);
    assert!(result.is_some());
}

#[test]
fn test_fix_markdown_json() {
    let json = r#"```json
{"name": "test"}
```"#;
    let result = fix_json(json);
    assert!(result.is_some());
}

#[test]
fn test_fix_truncated_json() {
    let json = r#"{"name": "test""#;
    let result = fix_json(json);
    assert!(result.is_some());
}

#[test]
fn test_fix_trailing_comma() {
    let json = r#"{"items": [1, 2, 3, ]}"#;
    let result = fix_json(json);
    assert!(result.is_some());
}
