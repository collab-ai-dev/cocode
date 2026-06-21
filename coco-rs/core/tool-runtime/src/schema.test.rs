//! Tests for the self-validating [`super::ToolInputSchema`] newtype (v4.2):
//! construction (`from_input_type` / `from_value`), composition-aware root
//! normalization, `schema_omit_properties`, `SchemaError` classification, and
//! the `resolve-http`-off build invariant. `super::` is the `schema` module.

use serde_json::{Value, json};

#[test]
fn from_input_type_closes_schema_and_validates() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[allow(dead_code)]
    struct DemoInput {
        name: String,
        count: Option<i64>,
    }
    let schema = super::ToolInputSchema::from_input_type::<DemoInput>();
    let v = schema.as_value();
    assert_eq!(v["type"], json!("object"));
    assert_eq!(
        v["additionalProperties"],
        json!(false),
        "internal schema must be closed"
    );
    assert!(schema.validate(&json!({"name": "x"})).is_ok());
    assert!(
        schema.validate(&json!({"name": "x", "bogus": 1})).is_err(),
        "additionalProperties:false must reject unknown fields"
    );
    assert!(
        schema.validate(&json!({})).is_err(),
        "missing required `name` must be rejected"
    );
}

#[test]
fn from_value_folds_in_missing_type() {
    let s = super::ToolInputSchema::from_value(json!({
        "properties": {"a": {"type": "string"}}
    }))
    .expect("typeless object schema accepted");
    assert_eq!(
        s.as_value()["type"],
        json!("object"),
        "type:object must be folded in"
    );
}

#[test]
fn from_value_rejects_explicit_non_object_roots() {
    assert!(super::ToolInputSchema::from_value(json!({"type": "array"})).is_err());
    assert!(super::ToolInputSchema::from_value(json!({"type": ["object", "null"]})).is_err());
    assert!(matches!(
        super::ToolInputSchema::from_value(json!({"type": "null"})),
        Err(super::SchemaError::RootTypeNull)
    ));
}

#[test]
fn from_value_does_not_fold_composition_root() {
    // A composition root ($ref) must not gain a spurious type:object.
    if let Ok(s) = super::ToolInputSchema::from_value(json!({
        "$ref": "#/$defs/X",
        "$defs": {"X": {"type": "object"}}
    })) {
        assert!(
            s.as_value().get("type").is_none(),
            "composition root must not gain type:object"
        );
    }
}

#[test]
fn from_value_rejects_remote_ref_without_fetch() {
    // Unknown-scheme $ref is rejected by jsonschema's retriever regardless of
    // the resolve-http feature, and never triggers a network request.
    let s = super::ToolInputSchema::from_value(json!({
        "type": "object",
        "properties": {"x": {"$ref": "made-up-scheme://nope"}}
    }));
    assert!(
        s.is_err(),
        "remote/unknown $ref must be rejected as Err, not fetched"
    );
}

#[test]
fn schema_omit_properties_removes_from_properties_and_required() {
    let base = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
        "required": ["a", "b"]
    });
    let out = super::schema_omit_properties(&base, &["b"]);
    assert!(out["properties"].get("b").is_none());
    assert_eq!(out["required"], json!(["a"]));
    assert!(
        base["properties"].get("b").is_some(),
        "input must be untouched"
    );
}

#[test]
fn schema_omit_properties_drops_empty_required() {
    let base = json!({
        "type": "object",
        "properties": {"a": {"type": "string"}},
        "required": ["a"]
    });
    let out = super::schema_omit_properties(&base, &["a"]);
    assert!(
        out.get("required").is_none(),
        "empty required must be removed"
    );
}

#[test]
fn canonicalize_model_tool_schema_stabilizes_key_and_required_order() {
    let a = super::canonicalize_model_tool_schema(&json!({
        "properties": {
            "b": {"type": "string", "description": "B"},
            "a": {"description": "A", "type": "string"}
        },
        "required": ["b", "a"]
    }));
    let b = super::canonicalize_model_tool_schema(&json!({
        "required": ["a", "b"],
        "properties": {
            "a": {"type": "string", "description": "A"},
            "b": {"description": "B", "type": "string"}
        }
    }));
    assert_eq!(a, b);
    assert_eq!(a["required"], json!(["a", "b"]));
    assert_eq!(a["type"], json!("object"));
}

#[test]
fn canonicalize_model_tool_schema_empty_schema_becomes_object() {
    assert_eq!(
        super::canonicalize_model_tool_schema(&json!({})),
        json!({"type": "object"})
    );
}

#[test]
fn canonicalize_model_tool_schema_drops_invalid_and_empty_required() {
    let invalid = super::canonicalize_model_tool_schema(&json!({
        "type": "object",
        "required": true,
        "properties": {"a": {"type": "string"}}
    }));
    assert!(invalid.get("required").is_none());

    let empty = super::canonicalize_model_tool_schema(&json!({
        "type": "object",
        "required": []
    }));
    assert!(empty.get("required").is_none());
}

#[test]
fn canonicalize_model_tool_schema_preserves_ordered_semantic_arrays() {
    let schema = json!({
        "type": "object",
        "properties": {
            "choice": {"enum": ["z", "a"]},
            "union": {"oneOf": [{"const": "b"}, {"const": "a"}]},
            "any": {"anyOf": [{"const": 2}, {"const": 1}]},
            "all": {"allOf": [{"required": ["b", "a"]}, {"required": ["d", "c"]}]}
        }
    });
    let out = super::canonicalize_model_tool_schema(&schema);
    assert_eq!(out["properties"]["choice"]["enum"], json!(["z", "a"]));
    assert_eq!(
        out["properties"]["union"]["oneOf"],
        json!([{"const": "b"}, {"const": "a"}])
    );
    assert_eq!(
        out["properties"]["any"]["anyOf"],
        json!([{"const": 2}, {"const": 1}])
    );
    assert_eq!(
        out["properties"]["all"]["allOf"],
        json!([{"required": ["a", "b"]}, {"required": ["c", "d"]}])
    );
}

#[test]
fn canonicalize_model_tool_schema_sorts_dependent_required() {
    let out = super::canonicalize_model_tool_schema(&json!({
        "type": "object",
        "dependentRequired": {
            "b": ["z", "a"],
            "a": ["d", "c"]
        }
    }));
    assert_eq!(out["dependentRequired"]["a"], json!(["c", "d"]));
    assert_eq!(out["dependentRequired"]["b"], json!(["a", "z"]));
}

#[test]
fn schema_error_classifies_invalid_arguments() {
    use coco_error::ErrorExt;
    let e = super::ToolInputSchema::from_value(json!({"type": "array"})).unwrap_err();
    assert_eq!(e.status_code(), coco_error::StatusCode::InvalidArguments);
    assert!(!e.is_retryable());
}

/// Build-invariant: jsonschema must stay `default-features = false` so a remote
/// `$ref` is rejected as `Err` (never fetched — SSRF / blocking-fetch guard).
/// Pure build-graph assertion via `cargo metadata`: no network, fails fast.
#[test]
fn jsonschema_resolve_http_stays_off() {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("cargo metadata");
    assert!(out.status.success(), "cargo metadata failed");
    let md: Value = serde_json::from_slice(&out.stdout).expect("metadata json");
    let node = md["resolve"]["nodes"]
        .as_array()
        .expect("resolve.nodes")
        .iter()
        .find(|n| n["id"].as_str().is_some_and(|s| s.contains("jsonschema@")))
        .expect("jsonschema in resolved graph");
    let feats: Vec<&str> = node["features"]
        .as_array()
        .expect("features array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        !feats.contains(&"resolve-http") && !feats.contains(&"resolve-file"),
        "jsonschema must stay default-features=false (SSRF/blocking-fetch guard); got {feats:?}",
    );
}

#[test]
fn validate_reports_every_unexpected_field() {
    // jsonschema lumps all unexpected keys into one error; `from_jsonschema`
    // must expand them so the model is told about every one in a single turn.
    let schema = super::ToolInputSchema::from_value(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {"name": {"type": "string"}},
        "required": ["name"]
    }))
    .expect("schema");
    let issues = schema
        .validate(&json!({"name": "x", "b1": 1, "b2": 2}))
        .expect_err("unexpected fields must be rejected");
    let unexpected: Vec<&str> = issues
        .iter()
        .filter_map(|i| match i {
            super::SchemaIssue::UnexpectedField { field, .. } => Some(field.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        unexpected.len(),
        2,
        "every unexpected key must surface: {issues:?}"
    );
    assert!(unexpected.contains(&"b1") && unexpected.contains(&"b2"));
}

#[test]
fn validate_enumerates_multiple_expected_types() {
    // A `type: [..]` mismatch must list the accepted types, not the literal
    // placeholder "multiple".
    let schema = super::ToolInputSchema::from_value(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {"x": {"type": ["string", "integer"]}}
    }))
    .expect("schema");
    let issues = schema
        .validate(&json!({"x": true}))
        .expect_err("bool must fail string|integer");
    let expected = issues
        .iter()
        .find_map(|i| match i {
            super::SchemaIssue::TypeMismatch { expected, .. } => Some(expected.clone()),
            _ => None,
        })
        .expect("a TypeMismatch issue");
    assert!(
        expected.contains("string") && expected.contains("integer"),
        "expected must enumerate both member types, got {expected:?}"
    );
    assert_ne!(
        expected, "multiple",
        "must not emit the literal placeholder"
    );
}
