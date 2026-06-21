use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn test_deep_merge_scalar_override() {
    let mut base = json!({"a": 1, "b": 2});
    let overlay = json!({"b": 3, "c": 4});
    deep_merge(&mut base, &overlay);
    assert_eq!(base, json!({"a": 1, "b": 3, "c": 4}));
}

#[test]
fn test_deep_merge_nested_objects() {
    let mut base = json!({"x": {"a": 1, "b": 2}});
    let overlay = json!({"x": {"b": 3, "c": 4}});
    deep_merge(&mut base, &overlay);
    assert_eq!(base, json!({"x": {"a": 1, "b": 3, "c": 4}}));
}

#[test]
fn test_deep_merge_arrays_concatenated() {
    let mut base = json!({"items": [1, 2]});
    let overlay = json!({"items": [3, 4]});
    deep_merge(&mut base, &overlay);
    assert_eq!(base, json!({"items": [1, 2, 3, 4]}));
}

#[test]
fn test_deep_merge_null_base() {
    let mut base = json!({"a": null});
    let overlay = json!({"a": "hello"});
    deep_merge(&mut base, &overlay);
    assert_eq!(base, json!({"a": "hello"}));
}
