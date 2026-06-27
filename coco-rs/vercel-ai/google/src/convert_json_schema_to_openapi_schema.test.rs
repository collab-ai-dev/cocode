use super::*;
use serde_json::json;

#[test]
fn converts_simple_object_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name"]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "object");
    assert_eq!(result["properties"]["name"]["type"], "string");
    assert_eq!(result["required"], json!(["name"]));
}

#[test]
fn converts_type_array_with_null() {
    // Gemini-3 strict: `anyOf` may not coexist with siblings (including
    // `nullable`). A single non-null type collapses to plain `type`.
    let schema = json!({
        "type": ["string", "null"]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "string");
    assert_eq!(result["nullable"], true);
    assert!(result.get("anyOf").is_none());
}

#[test]
fn converts_type_array_multiple_non_null() {
    let schema = json!({
        "type": ["string", "integer", "null"]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["anyOf"],
        json!([{ "type": "string" }, { "type": "integer" }])
    );
    assert_eq!(result["nullable"], true);
}

#[test]
fn converts_type_array_only_null() {
    let schema = json!({
        "type": ["null"]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "null");
}

#[test]
fn converts_const_to_enum() {
    let schema = json!({
        "const": "fixed_value"
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["enum"], json!(["fixed_value"]));
}

#[test]
fn converts_nested_properties() {
    let schema = json!({
        "type": "object",
        "properties": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" }
                }
            }
        }
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["properties"]["address"]["properties"]["street"]["type"],
        "string"
    );
}

#[test]
fn handles_truly_empty_schema() {
    let schema = json!({});
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result, json!({}));
}

#[test]
fn preserves_enum_values() {
    let schema = json!({
        "type": "string",
        "enum": ["a", "b", "c"]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["enum"], json!(["a", "b", "c"]));
}

// P2: Empty object schema handling
#[test]
fn empty_object_schema_returns_none_at_root() {
    let schema = json!({
        "type": "object",
        "properties": {}
    });
    assert!(convert_json_schema_to_openapi_schema(&schema).is_none());
}

#[test]
fn empty_object_schema_no_properties_returns_none_at_root() {
    let schema = json!({
        "type": "object"
    });
    assert!(convert_json_schema_to_openapi_schema(&schema).is_none());
}

#[test]
fn empty_object_schema_returns_type_object_when_nested() {
    let schema = json!({
        "type": "object",
        "properties": {
            "data": {
                "type": "object",
                "properties": {}
            }
        }
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["properties"]["data"], json!({ "type": "object" }));
}

#[test]
fn empty_object_schema_with_description_nested() {
    let schema = json!({
        "type": "object",
        "properties": {
            "data": {
                "type": "object",
                "properties": {},
                "description": "A data object"
            }
        }
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["properties"]["data"],
        json!({ "type": "object", "description": "A data object" })
    );
}

// P2: Boolean schema handling
#[test]
fn boolean_schema_true() {
    let schema = json!(true);
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result, json!({ "type": "boolean", "properties": {} }));
}

#[test]
fn boolean_schema_false() {
    let schema = json!(false);
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result, json!({ "type": "boolean", "properties": {} }));
}

// P2: anyOf null-type flattening
#[test]
fn anyof_with_null_single_schema_flattened() {
    let schema = json!({
        "anyOf": [
            { "type": "string" },
            { "type": "null" }
        ]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "string");
    assert_eq!(result["nullable"], true);
    assert!(result.get("anyOf").is_none());
}

#[test]
fn anyof_with_null_multiple_schemas_kept() {
    let schema = json!({
        "anyOf": [
            { "type": "string" },
            { "type": "integer" },
            { "type": "null" }
        ]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["anyOf"],
        json!([{ "type": "string" }, { "type": "integer" }])
    );
    assert_eq!(result["nullable"], true);
}

#[test]
fn anyof_without_null_passes_through() {
    let schema = json!({
        "anyOf": [
            { "type": "string" },
            { "type": "integer" }
        ]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["anyOf"],
        json!([{ "type": "string" }, { "type": "integer" }])
    );
    assert!(result.get("nullable").is_none());
}

/// Schemars emits this shape for `Option<i64>` with attributes:
/// `{anyOf: [{type: "integer"}], default, description, format, nullable}`.
/// Gemini-3 strict mode rejects `anyOf` alongside any sibling, with:
///   "When using any_of, it must be the only field set."
/// Flatten the single-element `anyOf` into the result.
#[test]
fn anyof_single_element_with_siblings_flattened() {
    let schema = json!({
        "anyOf": [{ "type": "integer" }],
        "default": null,
        "description": "line count",
        "format": "int64",
        "nullable": true,
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "integer");
    assert_eq!(result["description"], "line count");
    assert_eq!(result["format"], "int64");
    assert_eq!(result["nullable"], true);
    assert!(result.get("anyOf").is_none());
}

/// `Option<i64>` round-trip: input is `{type: ["integer", "null"], ...}`,
/// which goes through the type-array branch. Output must not contain
/// `anyOf` (would conflict with the sibling `description`/`format` carried
/// over by pass-through).
#[test]
fn option_int_type_array_with_siblings_flattened() {
    let schema = json!({
        "type": ["integer", "null"],
        "description": "limit",
        "format": "int64",
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "integer");
    assert_eq!(result["nullable"], true);
    assert_eq!(result["description"], "limit");
    assert_eq!(result["format"], "int64");
    assert!(result.get("anyOf").is_none());
}

/// Rust `schemars` emits this shape for a string enum with per-variant
/// docstrings (e.g. `GrepOutputMode`). Gemini-3 rejects the outer
/// schema because it lacks `type`. Coalesce into the canonical
/// `{type: "string", enum: [..]}` shape.
#[test]
fn oneof_singleton_enums_coalesced_to_string_enum() {
    let schema = json!({
        "oneOf": [
            { "description": "first",  "enum": ["content"],            "type": "string" },
            { "description": "second", "enum": ["files_with_matches"], "type": "string" },
            { "description": "third",  "enum": ["count"],              "type": "string" },
        ],
        "default": null,
        "description": "output mode",
        "nullable": true,
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "string");
    assert_eq!(
        result["enum"],
        json!(["content", "files_with_matches", "count"])
    );
    assert!(result.get("oneOf").is_none());
    assert_eq!(result["description"], "output mode");
    assert_eq!(result["nullable"], true);
}

/// Coalesce only fires when every alternative is `{enum: [v], type: T}`
/// with a consistent T. Mixed types or multi-value enums must stay as
/// verbatim `oneOf` so semantics are preserved.
#[test]
fn oneof_with_mixed_alternatives_preserved() {
    let schema = json!({
        "oneOf": [
            { "type": "string" },
            { "type": "integer" },
        ],
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(
        result["oneOf"],
        json!([{ "type": "string" }, { "type": "integer" }])
    );
    assert!(result.get("type").is_none());
    assert!(result.get("enum").is_none());
}

#[test]
fn anyof_with_null_complex_schema_flattened() {
    let schema = json!({
        "anyOf": [
            {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            },
            { "type": "null" }
        ]
    });
    let result = convert_json_schema_to_openapi_schema(&schema).unwrap();
    assert_eq!(result["type"], "object");
    assert_eq!(result["properties"]["name"]["type"], "string");
    assert_eq!(result["nullable"], true);
    assert!(result.get("anyOf").is_none());
}
