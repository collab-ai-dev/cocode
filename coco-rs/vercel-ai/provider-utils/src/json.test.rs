use super::*;

#[test]
fn test_parse_json_str() {
    let result: Value = parse_json_str(r#"{"key": "value"}"#).unwrap();
    assert_eq!(result["key"], "value");
}

#[test]
fn test_parse_json_secure() {
    let json = r#"{"a": {"b": {"c": 1}}}"#;
    let result: Value = parse_json_secure(json.as_bytes(), 10).unwrap();
    assert_eq!(result["a"]["b"]["c"], 1);
}

#[test]
fn test_parse_json_secure_depth_exceeded() {
    let json = r#"{"a": {"b": {"c": 1}}}"#;
    let result: Result<Value, _> = parse_json_secure(json.as_bytes(), 2);
    assert!(matches!(result, Err(SecureJsonError::DepthExceeded)));
}

#[test]
fn test_parse_json_event_stream() {
    let input = "data: {\"event\": \"test\"}\n\ndata: {\"event\": \"test2\"}\n\n";
    assert_eq!(parse_json_event_stream(input.as_bytes()).count(), 2);
}

// ─── merge_json_value: null-skip semantics (F10) ─────────────────────

#[test]
fn merge_null_overrides_preserves_base() {
    let base = serde_json::json!({"a": 1});
    let result = merge_json_value(&base, &Value::Null);
    assert_eq!(result, base);
}

#[test]
fn merge_null_at_existing_key_preserves_base() {
    let base = serde_json::json!({"a": 1, "b": 2});
    let over = serde_json::json!({"a": null});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": 1, "b": 2}));
}

#[test]
fn merge_null_at_new_key_is_skipped() {
    let base = serde_json::json!({"a": 1});
    let over = serde_json::json!({"b": null});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": 1}));
}

#[test]
fn merge_null_at_nested_subtree_preserves_base_subtree() {
    let base = serde_json::json!({"a": {"x": 1}});
    let over = serde_json::json!({"a": null});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": {"x": 1}}));
}

#[test]
fn merge_array_still_replaces() {
    let base = serde_json::json!({"a": [1, 2]});
    let over = serde_json::json!({"a": [3]});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": [3]}));
}

#[test]
fn merge_primitive_still_replaces() {
    let base = serde_json::json!({"a": 1});
    let over = serde_json::json!({"a": 2});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": 2}));
}

#[test]
fn merge_nested_object_recursive() {
    let base = serde_json::json!({"a": {"x": 1, "y": 2}});
    let over = serde_json::json!({"a": {"y": 99, "z": 3}});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": {"x": 1, "y": 99, "z": 3}}));
}

#[test]
fn merge_drops_prototype_polluting_keys() {
    let base = serde_json::json!({"a": 1});
    let over = serde_json::json!({"__proto__": {"hijack": true}, "b": 2});
    let result = merge_json_value(&base, &over);
    assert_eq!(result, serde_json::json!({"a": 1, "b": 2}));
}

#[test]
fn merge_null_in_thinking_config_path_preserves_typed_write() {
    // F10 doctrine: a typed channel writes
    // `body.generationConfig.thinkingConfig = {thinkingLevel: "high"}`
    // and the extras emit `{generationConfig: {thinkingConfig: null}}`
    // (e.g. user clears a default they don't want). Null-skip preserves
    // the typed write at every nesting depth — extras-as-eraser is
    // intentionally NOT supported (see merge_json_value doc).
    let base = serde_json::json!({
        "generationConfig": {
            "thinkingConfig": {"thinkingLevel": "high"}
        }
    });
    let over = serde_json::json!({
        "generationConfig": {
            "thinkingConfig": null
        }
    });
    let result = merge_json_value(&base, &over);
    assert_eq!(
        result,
        serde_json::json!({
            "generationConfig": {
                "thinkingConfig": {"thinkingLevel": "high"}
            }
        })
    );
}
