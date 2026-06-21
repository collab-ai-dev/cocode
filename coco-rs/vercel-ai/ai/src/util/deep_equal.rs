//! Deep equality check for data structures.
//!
//! This module provides utilities for deep equality comparison
//! of JSON values and other data structures.

use serde_json::Value;

/// Check if two JSON values are deeply equal.
pub fn is_deep_equal(a: &Value, b: &Value) -> bool {
    a == b
}

/// Check if two JSON values are deeply equal, ignoring key order in objects.
pub fn is_deep_equal_ignore_order(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(obj_a), Value::Object(obj_b)) => {
            if obj_a.len() != obj_b.len() {
                return false;
            }
            for (key, value_a) in obj_a {
                match obj_b.get(key) {
                    Some(value_b) => {
                        if !is_deep_equal_ignore_order(value_a, value_b) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        (Value::Array(arr_a), Value::Array(arr_b)) => {
            if arr_a.len() != arr_b.len() {
                return false;
            }
            for (item_a, item_b) in arr_a.iter().zip(arr_b.iter()) {
                if !is_deep_equal_ignore_order(item_a, item_b) {
                    return false;
                }
            }
            true
        }
        _ => a == b,
    }
}

/// Find differences between two JSON values.
#[derive(Debug, Clone, PartialEq)]
pub enum Difference {
    /// Types are different.
    TypeMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    /// Values are different.
    ValueMismatch {
        path: String,
        expected: Value,
        actual: Value,
    },
    /// Keys are different.
    KeyMismatch {
        path: String,
        missing_keys: Vec<String>,
        extra_keys: Vec<String>,
    },
    /// Array lengths are different.
    LengthMismatch {
        path: String,
        expected: usize,
        actual: usize,
    },
}

/// Compare two JSON values and return the differences.
pub fn find_differences(a: &Value, b: &Value, path: &str) -> Vec<Difference> {
    let mut differences = Vec::new();

    match (a, b) {
        (Value::Object(obj_a), Value::Object(obj_b)) => {
            // Check for missing/extra keys
            let keys_a: std::collections::HashSet<_> = obj_a.keys().collect();
            let keys_b: std::collections::HashSet<_> = obj_b.keys().collect();

            let missing: Vec<_> = keys_a.difference(&keys_b).map(|s| (*s).clone()).collect();
            let extra: Vec<_> = keys_b.difference(&keys_a).map(|s| (*s).clone()).collect();

            if !missing.is_empty() || !extra.is_empty() {
                differences.push(Difference::KeyMismatch {
                    path: path.to_string(),
                    missing_keys: missing,
                    extra_keys: extra,
                });
            }

            // Recursively check common keys
            for key in keys_a.intersection(&keys_b) {
                let new_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                differences.extend(find_differences(&obj_a[*key], &obj_b[*key], &new_path));
            }
        }
        (Value::Array(arr_a), Value::Array(arr_b)) => {
            if arr_a.len() != arr_b.len() {
                differences.push(Difference::LengthMismatch {
                    path: path.to_string(),
                    expected: arr_a.len(),
                    actual: arr_b.len(),
                });
            } else {
                for (i, (item_a, item_b)) in arr_a.iter().zip(arr_b.iter()).enumerate() {
                    let new_path = format!("{path}[{i}]");
                    differences.extend(find_differences(item_a, item_b, &new_path));
                }
            }
        }
        _ => {
            if a != b {
                differences.push(Difference::ValueMismatch {
                    path: path.to_string(),
                    expected: a.clone(),
                    actual: b.clone(),
                });
            }
        }
    }

    differences
}

#[cfg(test)]
#[path = "deep_equal.test.rs"]
mod tests;
