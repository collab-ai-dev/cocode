//! Tests for output.rs

use super::*;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct Person {
    name: String,
    age: u32,
}

#[test]
fn test_output_new() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });

    let output = Output::new(schema.clone());
    assert_eq!(output.schema, schema);
    assert!(output.name.is_none());
    assert!(output.description.is_none());
}

#[test]
fn test_output_with_name() {
    let schema = serde_json::json!({"type": "string"});
    let output = Output::new(schema).with_name("myOutput");

    assert_eq!(output.name, Some("myOutput".to_string()));
}

#[test]
fn test_output_from_type() {
    let output = Output::from_type::<Person>();
    assert!(output.schema.is_object());
}

#[test]
fn test_output_to_response_format() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let output = Output::new(schema).with_name("person");

    let format = output.to_response_format();
    // ResponseFormat should be JSON
    assert!(matches!(format, ResponseFormat::Json { .. }));
}

#[test]
fn test_output_mode_text() {
    let mode = OutputMode::text();
    assert!(mode.to_response_format().is_none());
}

#[test]
fn test_output_mode_object() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let mode = OutputMode::object(schema);
    assert!(mode.to_response_format().is_some());
}

#[test]
fn test_output_mode_object_from_type() {
    let mode = OutputMode::object_from_type::<Person>();
    assert!(mode.to_response_format().is_some());
}

#[test]
fn test_output_mode_array() {
    let element = serde_json::json!({"type": "string"});
    let mode = OutputMode::array(element);
    let format = mode.to_response_format();
    assert!(format.is_some());
}

#[test]
fn test_output_mode_parse_complete_text() {
    let mode = OutputMode::text();
    let result = mode.parse_complete_output("hello").unwrap();
    assert_eq!(result, Some(serde_json::Value::String("hello".to_string())));
}

#[test]
fn test_output_mode_parse_complete_object() {
    let mode = OutputMode::object(serde_json::json!({"type": "object"}));
    let result = mode.parse_complete_output(r#"{"name":"Alice"}"#).unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap()["name"], "Alice");
}

#[test]
fn test_output_mode_parse_partial_incomplete() {
    let mode = OutputMode::object(serde_json::json!({"type": "object"}));
    let result = mode.parse_partial_output(r#"{"name":"Ali"#).unwrap();
    // Partial JSON should return None since it's not parseable
    assert!(result.is_none());
}

// --- Phase 6: New output variants ---

#[test]
fn test_choice_output_valid() {
    let output = choice_output(vec![
        "yes".to_string(),
        "no".to_string(),
        "maybe".to_string(),
    ]);
    assert_eq!(output.name(), "choice");

    let result = output.parse_complete_output("yes").unwrap();
    assert_eq!(result, Some(serde_json::Value::String("yes".to_string())));
}

#[test]
fn test_choice_output_invalid() {
    let output = choice_output(vec!["yes".to_string(), "no".to_string()]);

    let result = output.parse_complete_output("banana").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_choice_output_json_string() {
    let output = choice_output(vec!["yes".to_string(), "no".to_string()]);

    let result = output.parse_complete_output(r#""yes""#).unwrap();
    assert_eq!(result, Some(serde_json::Value::String("yes".to_string())));
}

#[test]
fn test_choice_output_partial() {
    let output = choice_output(vec!["yes".to_string(), "no".to_string()]);

    let result = output.parse_partial_output("ye").unwrap();
    assert_eq!(result, Some(serde_json::Value::String("ye".to_string())));
}

#[test]
fn test_choice_output_response_format() {
    let output = choice_output(vec!["a".to_string(), "b".to_string()]);
    let format = output.response_format();
    assert!(format.is_some());
}

#[test]
fn test_json_output_valid() {
    let output = json_output();
    assert_eq!(output.name(), "json");

    let result = output.parse_complete_output(r#"{"key": "value"}"#).unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap()["key"], "value");
}

#[test]
fn test_json_output_array() {
    let output = json_output();

    let result = output.parse_complete_output(r#"[1, 2, 3]"#).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_json_output_invalid() {
    let output = json_output();

    let result = output.parse_complete_output("not json");
    assert!(result.is_err());
}

#[test]
fn test_json_output_response_format() {
    let output = json_output();
    let format = output.response_format();
    assert!(format.is_some());
}

#[test]
fn test_output_parse_context() {
    let output = text_output();
    let context = OutputParseContext {
        finish_reason: vercel_ai_provider::FinishReason::end_turn(),
        usage: vercel_ai_provider::Usage::default(),
    };
    let result = output
        .parse_complete_output_with_context("hello", &context)
        .unwrap();
    assert_eq!(result, Some(serde_json::Value::String("hello".to_string())));
}
