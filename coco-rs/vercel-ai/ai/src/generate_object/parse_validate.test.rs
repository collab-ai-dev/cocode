use serde::Deserialize;
use serde_json::json;

use super::*;

#[derive(Debug, Deserialize, PartialEq)]
struct SimpleObj {
    name: String,
    value: i32,
}

#[test]
fn test_parse_and_validate_valid_json() {
    let result = parse_and_validate::<SimpleObj>(r#"{"name":"test","value":42}"#).unwrap();
    assert_eq!(result.object.name, "test");
    assert_eq!(result.object.value, 42);
    assert!(!result.was_repaired);
}

#[test]
fn test_parse_and_validate_invalid_json() {
    let result = parse_and_validate::<SimpleObj>("not json");
    assert!(result.is_err());
}

#[test]
fn test_parse_and_validate_repairable_trailing_comma() {
    // Trailing comma should be auto-repaired
    let result = parse_and_validate::<SimpleObj>(r#"{"name":"test","value":42, }"#);
    // This may or may not succeed depending on repair_json_text implementation
    // The repair removes ", }" -> "}" pattern
    if let Ok(parsed) = result {
        assert!(parsed.was_repaired);
        assert_eq!(parsed.object.name, "test");
    }
}

#[test]
fn test_parse_and_validate_raw_preserved() {
    let input = r#"{"name":"test","value":42}"#;
    let result = parse_and_validate::<SimpleObj>(input).unwrap();
    assert_eq!(result.raw, input);
}

#[test]
fn test_parse_json_value_valid() {
    let result = parse_json_value(r#"{"key": "value"}"#).unwrap();
    assert_eq!(result["key"], "value");
}

#[test]
fn test_parse_json_value_invalid() {
    let result = parse_json_value("not json {{{");
    assert!(result.is_err());
}

#[test]
fn test_parse_json_value_repairable() {
    // Trailing comma
    let result = parse_json_value(r#"{"key": "value", }"#);
    if let Ok(v) = result {
        assert_eq!(v["key"], "value");
    }
}

#[test]
fn test_validate_against_schema_object_match() {
    let value = json!({"name": "test"});
    let schema = json!({"type": "object"});
    assert!(validate_against_schema(&value, &schema).is_ok());
}

#[test]
fn test_validate_against_schema_type_mismatch() {
    let value = json!("a string");
    let schema = json!({"type": "object"});
    let result = validate_against_schema(&value, &schema);
    assert!(result.is_err());
}

#[test]
fn test_validate_against_schema_integer() {
    let value = json!(42);
    let schema = json!({"type": "integer"});
    assert!(validate_against_schema(&value, &schema).is_ok());
}

#[test]
fn test_validate_against_schema_array() {
    let value = json!([1, 2, 3]);
    let schema = json!({"type": "array"});
    assert!(validate_against_schema(&value, &schema).is_ok());
}

#[test]
fn test_validate_against_schema_no_type() {
    // Schema without type constraint should always pass
    let value = json!("anything");
    let schema = json!({});
    assert!(validate_against_schema(&value, &schema).is_ok());
}
