use super::*;
use serde_json::json;

#[test]
fn strips_unsupported_keywords() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "count": { "type": "integer", "minimum": 0, "maximum": 100 }
        },
        "required": ["name"]
    });

    let out = sanitize_json_schema(&schema);
    assert_eq!(out["type"], "object");
    assert!(out["properties"]["name"].get("minLength").is_none());
    assert!(out["properties"]["count"].get("minimum").is_none());
    let desc = out["properties"]["name"]["description"].as_str().unwrap();
    assert!(desc.contains("min length: 1"));
}

#[test]
fn keeps_ref_only() {
    let schema = json!({ "$ref": "#/definitions/Foo", "description": "ignored" });
    let out = sanitize_json_schema(&schema);
    assert_eq!(out["$ref"], "#/definitions/Foo");
    assert!(out.get("description").is_none());
}

#[test]
fn converts_one_of_to_any_of() {
    let schema = json!({ "oneOf": [{"type": "string"}, {"type": "number"}] });
    let out = sanitize_json_schema(&schema);
    assert!(out.get("anyOf").is_some());
    assert!(out.get("oneOf").is_none());
}

#[test]
fn allowed_string_format_kept() {
    let schema = json!({ "type": "string", "format": "date-time" });
    let out = sanitize_json_schema(&schema);
    assert_eq!(out["format"], "date-time");
}

#[test]
fn disallowed_format_moved_to_description() {
    let schema = json!({ "type": "string", "format": "binary" });
    let out = sanitize_json_schema(&schema);
    assert!(out.get("format").is_none());
    let desc = out["description"].as_str().unwrap();
    assert!(desc.contains("format: binary"));
}

#[test]
fn adds_additional_properties_false_for_objects() {
    let schema = json!({
        "type": "object",
        "properties": { "x": { "type": "number" } }
    });
    let out = sanitize_json_schema(&schema);
    assert_eq!(out["additionalProperties"], false);
}
