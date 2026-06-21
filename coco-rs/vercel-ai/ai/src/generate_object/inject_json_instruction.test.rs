//! Tests for inject_json_instruction.rs

use super::*;
use serde_json::json;

#[test]
fn test_inject_json_instruction_with_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });

    let result = inject_json_instruction(Some("Generate a person"), Some(&schema));

    assert!(result.contains("Generate a person"));
    assert!(result.contains("JSON schema"));
    assert!(result.contains("You MUST answer with a JSON object"));
}

#[test]
fn test_inject_json_instruction_without_schema() {
    let result = inject_json_instruction(Some("Generate something"), None);

    assert!(result.contains("Generate something"));
    assert!(result.contains("You MUST answer with JSON"));
}
