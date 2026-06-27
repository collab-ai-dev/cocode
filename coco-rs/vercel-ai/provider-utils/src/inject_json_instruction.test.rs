//! Tests for inject_json_instruction module.

use super::*;
use serde_json::json;

#[test]
fn test_inject_json_instruction_basic() {
    let prompt = "Extract the user's name.";
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });

    let result = inject_json_instruction(prompt, &schema);

    assert!(result.starts_with(prompt));
    assert!(result.contains("JSON"));
    assert!(result.contains("schema"));
}

#[test]
fn test_inject_json_instruction_contains_schema() {
    let prompt = "Test prompt";
    let schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer" }
        }
    });

    let result = inject_json_instruction(prompt, &schema);

    // Should contain the schema content
    assert!(result.contains("\"type\": \"object\""));
    assert!(result.contains("\"id\""));
}

#[test]
fn test_inject_json_instruction_with_description() {
    let prompt = "Parse the document.";
    let schema = json!({ "type": "string" });
    let description = "Return the document title.";

    let result = inject_json_instruction_with_description(prompt, &schema, description);

    assert!(result.contains(prompt));
    assert!(result.contains(description));
    assert!(result.contains("Schema:"));
}

#[test]
fn test_create_json_response_instruction() {
    let schema = json!({
        "type": "object",
        "properties": {
            "status": { "type": "string" }
        }
    });

    let result = create_json_response_instruction(&schema);

    assert!(result.contains("JSON object"));
    assert!(result.contains("schema"));
    assert!(result.contains("valid JSON"));
}

#[test]
fn test_inject_json_array_instruction() {
    let prompt = "List all users.";
    let item_schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "email": { "type": "string" }
        }
    });

    let result = inject_json_array_instruction(prompt, &item_schema);

    assert!(result.starts_with(prompt));
    assert!(result.contains("JSON array"));
    assert!(result.contains("\"name\""));
    assert!(result.contains("\"email\""));
}

#[test]
fn test_inject_json_instruction_complex_schema() {
    let prompt = "Analyze the text.";
    let schema = json!({
        "type": "object",
        "properties": {
            "sentiment": {
                "type": "string",
                "enum": ["positive", "negative", "neutral"]
            },
            "confidence": {
                "type": "number",
                "minimum": 0,
                "maximum": 1
            },
            "keywords": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["sentiment", "confidence"]
    });

    let result = inject_json_instruction(prompt, &schema);

    assert!(result.contains("sentiment"));
    assert!(result.contains("confidence"));
    assert!(result.contains("keywords"));
}
