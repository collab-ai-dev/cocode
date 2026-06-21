use super::*;
use serde_json::json;

#[test]
fn test_validate_object_generation_input_auto() {
    let schema = json!({ "type": "object" });
    let result = validate_object_generation_input(&schema, ObjectGenerationMode::Auto);
    assert!(result.is_ok());
}

#[test]
fn test_validate_object_generation_input_json() {
    let schema = json!({ "type": "string" });
    let result = validate_object_generation_input(&schema, ObjectGenerationMode::Json);
    assert!(result.is_ok());
}

#[test]
fn test_validate_object_generation_input_tool() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let result = validate_object_generation_input(&schema, ObjectGenerationMode::Tool);
    assert!(result.is_ok());
}

#[test]
fn test_validate_object_schema_invalid_properties() {
    let schema = json!({
        "type": "object",
        "properties": "invalid"
    });
    let result = validate_object_schema(&schema);
    assert!(result.is_err());
}

#[test]
fn test_validate_grammar_schema_null() {
    let schema = json!(null);
    let result = validate_grammar_schema(&schema);
    assert!(result.is_err());
}

#[test]
fn test_determine_generation_mode() {
    // Prefer tools
    assert_eq!(
        determine_generation_mode(true, true, true),
        ObjectGenerationMode::Tool
    );

    // Fall back to JSON
    assert_eq!(
        determine_generation_mode(true, false, true),
        ObjectGenerationMode::Json
    );

    // Fall back to grammar
    assert_eq!(
        determine_generation_mode(false, false, true),
        ObjectGenerationMode::Grammar
    );

    // Default to auto
    assert_eq!(
        determine_generation_mode(false, false, false),
        ObjectGenerationMode::Auto
    );
}