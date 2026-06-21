//! Value extraction utility.
//!
//! This module provides utilities for extracting values from various types.

use serde_json::Value as JSONValue;

/// Extract a value from a JSON value.
///
/// # Arguments
///
/// * `value` - The JSON value.
///
/// # Returns
///
/// The extracted value.
pub fn value_of<T: FromJSONValue>(value: &JSONValue) -> Option<T> {
    T::from_json(value)
}

/// Trait for types that can be extracted from JSON values.
pub trait FromJSONValue: Sized {
    /// Extract the value from a JSON value.
    fn from_json(value: &JSONValue) -> Option<Self>;
}

impl FromJSONValue for String {
    fn from_json(value: &JSONValue) -> Option<Self> {
        value.as_str().map(std::string::ToString::to_string)
    }
}

impl FromJSONValue for i64 {
    fn from_json(value: &JSONValue) -> Option<Self> {
        value.as_i64()
    }
}

impl FromJSONValue for f64 {
    fn from_json(value: &JSONValue) -> Option<Self> {
        value.as_f64()
    }
}

impl FromJSONValue for bool {
    fn from_json(value: &JSONValue) -> Option<Self> {
        value.as_bool()
    }
}

impl FromJSONValue for JSONValue {
    fn from_json(value: &JSONValue) -> Option<Self> {
        Some(value.clone())
    }
}

/// Extract a nested value from a JSON object.
///
/// # Arguments
///
/// * `value` - The JSON value.
/// * `path` - The path to the nested value (e.g., "a.b.c").
///
/// # Returns
///
/// The nested value if found.
pub fn get_nested_value<'a>(value: &'a JSONValue, path: &str) -> Option<&'a JSONValue> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        match current {
            JSONValue::Object(obj) => {
                current = obj.get(part)?;
            }
            JSONValue::Array(arr) => {
                let index: usize = part.parse().ok()?;
                current = arr.get(index)?;
            }
            _ => return None,
        }
    }

    Some(current)
}

/// Extract a nested value and convert to a specific type.
///
/// # Arguments
///
/// * `value` - The JSON value.
/// * `path` - The path to the nested value.
///
/// # Returns
///
/// The extracted value if found and convertible.
pub fn get_nested<T: FromJSONValue>(value: &JSONValue, path: &str) -> Option<T> {
    get_nested_value(value, path).and_then(|v| T::from_json(v))
}
