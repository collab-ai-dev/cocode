use super::*;
use serde_json::json;

#[test]
fn adds_additional_properties_false_to_root_object() {
    let mut schema = json!({
        "type": "object",
        "properties": { "name": { "type": "string" } }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn recurses_into_nested_object_properties() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "user": {
                "type": "object",
                "properties": { "id": { "type": "integer" } }
            }
        }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(
        schema["properties"]["user"]["additionalProperties"],
        json!(false)
    );
}

#[test]
fn recurses_into_array_items_object() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": { "type": "object", "properties": {} }
            }
        }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(
        schema["properties"]["items"]["items"]["additionalProperties"],
        json!(false)
    );
}

#[test]
fn recurses_into_any_all_one_of() {
    let mut schema = json!({
        "anyOf": [
            { "type": "object", "properties": {} },
            { "type": "string" }
        ]
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(schema["anyOf"][0]["additionalProperties"], json!(false));
}

#[test]
fn handles_array_type_including_object() {
    let mut schema = json!({
        "type": ["object", "null"],
        "properties": { "x": { "type": "string" } }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn skips_non_object_root() {
    let mut schema = json!({ "type": "string" });
    add_additional_properties_to_json_schema(&mut schema);
    assert!(schema.get("additionalProperties").is_none());
}

#[test]
fn handles_definitions() {
    let mut schema = json!({
        "definitions": {
            "User": { "type": "object", "properties": {} }
        }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(
        schema["definitions"]["User"]["additionalProperties"],
        json!(false)
    );
}

#[test]
fn boolean_schema_left_alone() {
    let mut schema = json!({
        "type": "object",
        "properties": { "any": true }
    });
    add_additional_properties_to_json_schema(&mut schema);
    assert_eq!(schema["properties"]["any"], json!(true));
}
