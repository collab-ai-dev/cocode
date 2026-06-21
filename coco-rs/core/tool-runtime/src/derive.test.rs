//! Tests for `derive_input_schema_value`.
//!
//! Each test fixes one expected behaviour of the derive helpers so the
//! tool migrations that depend on this module land on a stable contract.
//! If schemars changes its output shape across versions these tests catch
//! it before it cascades through the tool surface.
//!
//! `derive_input_schema_value` returns the full JSON Schema document, so
//! assertions read `["properties"]` / `["required"]` off the `Value`.
//!
//! Test naming: `test_<concern>_<expected_outcome>`.

use super::*;
use pretty_assertions::assert_eq;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

// ──────────────────────────────────────────────────────────────────
// Accessors over the derived full-document `Value`
// ──────────────────────────────────────────────────────────────────

/// The `required` list as owned strings (empty when the key is absent).
fn required_list(schema: &serde_json::Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The `properties` object (`None` when the key is absent).
fn properties(schema: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
}

// ──────────────────────────────────────────────────────────────────
// Fixture types
// ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct SimpleInput {
    /// The glob pattern to match files against
    pattern: String,
    /// The directory to search in.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct WithDefault {
    pattern: String,
    /// Defaults to 100 when omitted.
    #[serde(default)]
    limit: i32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum OutputMode {
    Content,
    FilesWithMatches,
    Count,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct WithEnum {
    pattern: String,
    mode: OutputMode,
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Nested {
    inner_a: String,
    inner_b: i32,
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct WithNested {
    outer: String,
    nested: Nested,
}

// ──────────────────────────────────────────────────────────────────
// derive_input_schema_value — basics
// ──────────────────────────────────────────────────────────────────

#[test]
fn test_required_field_appears_in_required_list() {
    let schema = derive_input_schema_value::<SimpleInput>();
    let required = required_list(&schema);
    assert!(
        required.contains(&"pattern".to_string()),
        "expected `pattern` in required, got: {required:?}"
    );
}

#[test]
fn test_option_field_not_in_required_list() {
    let schema = derive_input_schema_value::<SimpleInput>();
    let required = required_list(&schema);
    assert!(
        !required.contains(&"path".to_string()),
        "`path: Option<String>` must not appear in required, got: {required:?}"
    );
}

#[test]
fn test_serde_default_field_not_in_required_list() {
    // `#[serde(default)] limit: i32` — value-typed but optional via
    // serde default. Schemars marks fields not in `required` when
    // `#[serde(default)]` is present.
    let schema = derive_input_schema_value::<WithDefault>();
    let required = required_list(&schema);
    assert!(
        !required.contains(&"limit".to_string()),
        "`#[serde(default)]` field must not appear in required, got: {required:?}"
    );
}

#[test]
fn test_properties_keys_present_for_all_fields() {
    let schema = derive_input_schema_value::<SimpleInput>();
    let props = properties(&schema).expect("object schema has properties");
    assert!(props.contains_key("pattern"));
    assert!(props.contains_key("path"));
}

#[test]
fn test_field_description_propagates_from_doc_comment() {
    let schema = derive_input_schema_value::<SimpleInput>();
    let pattern_schema = properties(&schema)
        .and_then(|p| p.get("pattern"))
        .expect("pattern property must exist");
    let description = pattern_schema
        .get("description")
        .and_then(|v| v.as_str())
        .expect("pattern must have a description from its /// comment");
    assert_eq!(description, "The glob pattern to match files against");
}

// ──────────────────────────────────────────────────────────────────
// Enum field encoding
// ──────────────────────────────────────────────────────────────────

#[test]
fn test_enum_field_emits_enum_values_in_schema() {
    let schema = derive_input_schema_value::<WithEnum>();
    let mode_schema = properties(&schema)
        .and_then(|p| p.get("mode"))
        .expect("mode property must exist");
    let enum_values = mode_schema
        .get("enum")
        .and_then(|v| v.as_array())
        .expect("mode field with enum type must produce `enum: [...]` in schema");
    let values: Vec<&str> = enum_values.iter().filter_map(|v| v.as_str()).collect();
    // snake_case rename applies — variants are emitted as wire strings.
    assert!(values.contains(&"content"));
    assert!(values.contains(&"files_with_matches"));
    assert!(values.contains(&"count"));
}

// ──────────────────────────────────────────────────────────────────
// Subschema inlining — the load-bearing behaviour
// ──────────────────────────────────────────────────────────────────

#[test]
fn test_nested_struct_is_inlined_not_referenced() {
    // The critical invariant: a nested type must NOT appear as
    // `$ref: "#/$defs/Nested"` — providers handle $ref inconsistently
    // and schemas must not produce one. We require the inner
    // properties to be flattened into the parent.
    let value = derive_input_schema_value::<WithNested>();

    let serialized = serde_json::to_string(&value).unwrap();
    assert!(
        !serialized.contains("$ref"),
        "nested types must be inlined, but schema still contains $ref: {serialized}"
    );
    assert!(
        !serialized.contains("$defs"),
        "$defs table must be empty/absent after inlining, got: {serialized}"
    );

    // The `nested` field's schema must itself be an object schema
    // carrying the inner fields directly.
    let nested = value
        .get("properties")
        .and_then(|v| v.get("nested"))
        .expect("`nested` property must be present");
    let inner_props = nested
        .get("properties")
        .expect("inlined nested struct must expose its properties directly");
    assert!(inner_props.get("inner_a").is_some());
    assert!(inner_props.get("inner_b").is_some());
}

// ──────────────────────────────────────────────────────────────────
// snake_case wire format
// ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct SnakeCaseInput {
    pattern: String,
    /// Note the `runInBackground` Rust ident would be camelCase by
    /// default; the `rename_all = "snake_case"` collapses to
    /// `run_in_background`.
    #[serde(default)]
    run_in_background: bool,
}

#[test]
fn test_snake_case_rename_applies_to_property_keys() {
    let schema = derive_input_schema_value::<SnakeCaseInput>();
    let props = properties(&schema).expect("object schema has properties");
    assert!(props.contains_key("run_in_background"));
    assert!(props.contains_key("pattern"));
}

// ──────────────────────────────────────────────────────────────────
// derive_input_schema_value — full envelope path
// ──────────────────────────────────────────────────────────────────

#[test]
fn test_input_schema_value_returns_full_object_envelope() {
    // The full Value path keeps the `type: object` envelope — this is
    // what `ToolInputSchema::from_input_type` closes and compiles.
    let value = derive_input_schema_value::<SimpleInput>();
    assert_eq!(value.get("type"), Some(&json!("object")));
    assert!(value.get("properties").is_some());
}

// ──────────────────────────────────────────────────────────────────
// Empty struct → empty schema (degenerate but legal)
// ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct EmptyInput {}

#[test]
fn test_empty_struct_yields_empty_properties_and_required() {
    let schema = derive_input_schema_value::<EmptyInput>();
    assert!(
        properties(&schema).is_none_or(serde_json::Map::is_empty),
        "empty struct must yield no properties, got: {schema}"
    );
    assert!(
        required_list(&schema).is_empty(),
        "empty struct must yield no required entries, got: {schema}"
    );
}
