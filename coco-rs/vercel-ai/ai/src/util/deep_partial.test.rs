use super::*;
use serde_json::json;

#[test]
fn test_deep_partial_complete() {
    let partial: DeepPartial<i32> = DeepPartial::complete(42);
    assert!(partial.is_complete());
    assert_eq!(partial.as_complete(), Some(&42));
}

#[test]
fn test_deep_partial_partial() {
    let partial: DeepPartial<i32> = DeepPartial::partial(json!({ "value": 42 }));
    assert!(partial.is_partial());
}

#[test]
fn test_deep_partial_missing() {
    let partial: DeepPartial<i32> = DeepPartial::missing();
    assert!(partial.is_missing());
}

#[test]
fn test_merge_partial_json() {
    let base = json!({ "a": 1, "b": 2 });
    let update = json!({ "b": 3, "c": 4 });
    let merged = merge_partial_json(&base, &update);

    assert_eq!(merged["a"], 1);
    assert_eq!(merged["b"], 3);
    assert_eq!(merged["c"], 4);
}

#[test]
fn test_is_partial_object() {
    let value = json!({ "a": 1, "b": 2 });
    assert!(!is_partial_object(&value, &["a", "b"]));
    assert!(is_partial_object(&value, &["a", "c"]));
}