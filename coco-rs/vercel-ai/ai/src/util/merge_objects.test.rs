use super::*;
use serde_json::json;

#[test]
fn test_both_none() {
    let result = merge_objects(None, None);
    assert!(result.is_none());
}

#[test]
fn test_base_none() {
    let result = merge_objects(None, Some(json!({"a": 1})));
    assert_eq!(result, Some(json!({"a": 1})));
}

#[test]
fn test_overrides_none() {
    let result = merge_objects(Some(json!({"a": 1})), None);
    assert_eq!(result, Some(json!({"a": 1})));
}

#[test]
fn test_simple_merge() {
    let base = json!({"a": 1, "b": 2});
    let overrides = json!({"b": 3, "c": 4});
    let result = merge_objects(Some(base), Some(overrides));
    assert_eq!(result, Some(json!({"a": 1, "b": 3, "c": 4})));
}

#[test]
fn test_nested_merge() {
    let base = json!({"a": {"x": 1, "y": 2}});
    let overrides = json!({"a": {"y": 3, "z": 4}});
    let result = merge_objects(Some(base), Some(overrides));
    assert_eq!(result, Some(json!({"a": {"x": 1, "y": 3, "z": 4}})));
}

#[test]
fn test_array_replacement() {
    let base = json!({"a": [1, 2, 3]});
    let overrides = json!({"a": [4, 5]});
    let result = merge_objects(Some(base), Some(overrides));
    assert_eq!(result, Some(json!({"a": [4, 5]})));
}

#[test]
fn test_null_in_overrides_kept() {
    // In JSON, null is a valid value that should override the base value
    // (unlike an absent/undefined key in other languages)
    let base = json!({"a": 1, "b": 2});
    let overrides = json!({"b": null, "c": 3});
    let result = merge_objects(Some(base), Some(overrides));
    // null overrides 2, and 3 is added
    assert_eq!(result, Some(json!({"a": 1, "b": null, "c": 3})));
}

#[test]
fn test_non_object_override() {
    let base = json!({"a": 1});
    let overrides = json!("string");
    let result = merge_objects(Some(base), Some(overrides));
    assert_eq!(result, Some(json!("string")));
}
