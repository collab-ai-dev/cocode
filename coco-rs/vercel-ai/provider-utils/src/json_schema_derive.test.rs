//! Tests for json_schema_derive module.

use super::*;

#[test]
fn test_generated_schema_new() {
    let schema = JSONValue::Object(serde_json::Map::new());
    let generated = GeneratedSchema::new(schema.clone());
    assert_eq!(generated.as_json(), &schema);
}

#[test]
fn test_generated_schema_to_string_pretty() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let generated = GeneratedSchema::new(schema);
    let s = generated.to_string_pretty();
    assert!(s.contains("\"type\""));
    assert!(s.contains("\"name\""));
}

#[test]
#[cfg(feature = "json-schema")]
fn test_schema_from_type() {
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(JsonSchema, Deserialize)]
    #[allow(dead_code)]
    struct TestStruct {
        name: String,
        count: i32,
    }

    let schema = schema_from_type::<TestStruct>();
    let json = schema.as_json();

    // Should have type: object
    assert!(json.get("type").is_some());
}

#[test]
#[cfg(not(feature = "json-schema"))]
fn test_schema_from_type_stub() {
    let schema = schema_from_type::<()>();
    // Should return empty object when feature is disabled
    assert!(schema.as_json().is_object());
}

#[test]
fn test_merge_into_schema() {
    let schema = JSONValue::Object(serde_json::Map::new());
    let mut generated = GeneratedSchema::new(schema);

    merge_into_schema(
        &mut generated,
        "custom_field",
        JSONValue::String("value".to_string()),
    );

    assert_eq!(
        generated.as_json().get("custom_field"),
        Some(&JSONValue::String("value".to_string()))
    );
}

#[test]
fn test_add_required_fields() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "email": { "type": "string" }
        }
    });
    let mut generated = GeneratedSchema::new(schema);

    add_required_fields(&mut generated, &["name", "email"]);

    let required = generated.as_json().get("required").unwrap();
    assert!(required.is_array());

    let arr = required.as_array().unwrap();
    assert!(arr.contains(&JSONValue::String("name".to_string())));
    assert!(arr.contains(&JSONValue::String("email".to_string())));
}
