use super::to_openai_compatible_schema;
use serde_json::json;

#[test]
fn one_of_is_rewritten_to_any_of() {
    let out = to_openai_compatible_schema(&json!({
        "type": "object",
        "properties": {
            "x": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
        }
    }));
    assert!(
        out["properties"]["x"].get("oneOf").is_none(),
        "oneOf must be gone"
    );
    let any_of = out["properties"]["x"]["anyOf"]
        .as_array()
        .expect("anyOf array");
    assert_eq!(any_of.len(), 2);
}

#[test]
fn one_of_merges_into_existing_any_of_without_dropping_branches() {
    let out = to_openai_compatible_schema(&json!({
        "anyOf": [{"type": "string"}],
        "oneOf": [{"type": "integer"}]
    }));
    assert!(out.get("oneOf").is_none());
    let any_of = out["anyOf"].as_array().expect("anyOf array");
    assert_eq!(any_of.len(), 2, "both union branches must survive: {out}");
}

#[test]
fn safe_all_of_of_disjoint_objects_is_flattened() {
    let out = to_openai_compatible_schema(&json!({
        "allOf": [
            {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
            {"type": "object", "properties": {"b": {"type": "integer"}}}
        ]
    }));
    assert!(out.get("allOf").is_none(), "safe allOf must flatten: {out}");
    assert_eq!(out["type"], json!("object"));
    assert_eq!(out["properties"]["a"]["type"], json!("string"));
    assert_eq!(out["properties"]["b"]["type"], json!("integer"));
    assert_eq!(out["required"], json!(["a"]));
}

#[test]
fn lossy_all_of_with_ref_is_left_verbatim() {
    let input = json!({
        "type": "object",
        "allOf": [
            {"$ref": "#/$defs/Base"},
            {"type": "object", "properties": {"b": {"type": "integer"}}}
        ],
        "$defs": {"Base": {"type": "object"}}
    });
    let out = to_openai_compatible_schema(&input);
    assert!(
        out.get("allOf").is_some(),
        "a $ref branch must not be flattened: {out}"
    );
}

#[test]
fn lossy_all_of_with_overlapping_property_is_left_verbatim() {
    let input = json!({
        "allOf": [
            {"type": "object", "properties": {"x": {"type": "string"}}},
            {"type": "object", "properties": {"x": {"type": "integer"}}}
        ]
    });
    let out = to_openai_compatible_schema(&input);
    assert!(
        out.get("allOf").is_some(),
        "overlapping property must block the merge: {out}"
    );
}

#[test]
fn transform_is_idempotent() {
    let input = json!({
        "type": "object",
        "properties": {
            "x": {"oneOf": [{"type": "string"}, {"type": "null"}]}
        },
        "allOf": [
            {"type": "object", "properties": {"a": {"type": "string"}}}
        ]
    });
    let once = to_openai_compatible_schema(&input);
    let twice = to_openai_compatible_schema(&once);
    assert_eq!(once, twice, "transform must be idempotent");
}

#[test]
fn plain_object_schema_is_unchanged() {
    let input = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {"a": {"type": "string"}},
        "required": ["a"]
    });
    assert_eq!(to_openai_compatible_schema(&input), input);
}

#[test]
fn never_forces_additional_properties() {
    // An external schema's open `additionalProperties` is preserved...
    let open = json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {"a": {"type": "string"}}
    });
    assert_eq!(
        to_openai_compatible_schema(&open)["additionalProperties"],
        json!(true)
    );

    // ...and a schema without it does not gain one.
    let bare = json!({"type": "object", "properties": {"a": {"type": "string"}}});
    assert!(
        to_openai_compatible_schema(&bare)
            .get("additionalProperties")
            .is_none()
    );
}

#[test]
fn any_of_only_schema_is_passed_through() {
    // No `oneOf` ⇒ the union is left exactly as-is (no rebuild / reorder).
    let input = json!({
        "type": "object",
        "properties": {"x": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}
    });
    assert_eq!(to_openai_compatible_schema(&input), input);
}

#[test]
fn union_free_schema_is_byte_identical_for_prompt_cache_safety() {
    // The transform must be byte-identical (key order preserved), not merely
    // `Value`-equal, for any schema without oneOf/allOf — i.e. every coco tool.
    // With serde_json `preserve_order` on, a reordering rebuild would shift the
    // tools-array prefix and break the provider prompt cache. Keys are
    // deliberately NOT in alphabetical order to prove insertion order survives.
    let input = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "zebra": {"type": "string"},
            "alpha": {"type": "integer"},
            "nested": {
                "type": "object",
                "properties": {"y": {"type": "boolean"}, "x": {"type": "string"}}
            }
        },
        "required": ["zebra", "alpha"]
    });
    assert_eq!(
        serde_json::to_string(&to_openai_compatible_schema(&input)).unwrap(),
        serde_json::to_string(&input).unwrap(),
        "union-free transform must preserve key order byte-for-byte"
    );
}

#[test]
fn property_named_like_a_composition_keyword_is_not_treated_as_a_union() {
    // Property names that collide with composition keywords must survive
    // verbatim — keyword-aware recursion must not misread a `properties` map.
    let input = json!({
        "type": "object",
        "properties": {
            "oneOf": {"type": "string"},
            "anyOf": {"type": "integer"}
        }
    });
    assert_eq!(to_openai_compatible_schema(&input), input);
}

#[test]
fn composition_keywords_inside_a_value_are_left_verbatim() {
    // A `default`/`const`/`enum` value that happens to contain `oneOf` is data,
    // not a subschema, and must not be rewritten.
    let input = json!({
        "type": "object",
        "properties": {
            "x": {"type": "object", "default": {"oneOf": "not a schema"}}
        }
    });
    assert_eq!(to_openai_compatible_schema(&input), input);
}
