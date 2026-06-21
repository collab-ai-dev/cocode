//! Tests for deep_equal.rs

use super::*;
use serde_json::json;

#[test]
fn test_equal_objects() {
    let a = json!({"name": "test", "value": 42});
    let b = json!({"name": "test", "value": 42});
    assert!(is_deep_equal(&a, &b));
}

#[test]
fn test_different_values() {
    let a = json!({"name": "test"});
    let b = json!({"name": "other"});
    assert!(!is_deep_equal(&a, &b));
}

#[test]
fn test_equal_arrays() {
    let a = json!([1, 2, 3]);
    let b = json!([1, 2, 3]);
    assert!(is_deep_equal(&a, &b));
}

#[test]
fn test_find_differences() {
    let a = json!({"name": "test", "value": 42});
    let b = json!({"name": "test", "value": 43, "extra": true});
    let diffs = find_differences(&a, &b, "");
    assert!(!diffs.is_empty());
}
