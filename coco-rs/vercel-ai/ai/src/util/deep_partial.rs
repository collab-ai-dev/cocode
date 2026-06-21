//! Deep partial type utilities.
//!
//! This module provides utilities for working with partial types
//! where some fields may be missing.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JSONValue;

/// A deep partial value that may have missing fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeepPartial<T> {
    /// A complete value.
    Complete(T),
    /// A partial value with some fields missing.
    Partial(JSONValue),
    /// Missing entirely.
    #[default]
    Missing,
}

impl<T> DeepPartial<T> {
    /// Create a complete value.
    pub fn complete(value: T) -> Self {
        Self::Complete(value)
    }

    /// Create a partial value.
    pub fn partial(value: JSONValue) -> Self {
        Self::Partial(value)
    }

    /// Create a missing value.
    pub fn missing() -> Self {
        Self::Missing
    }

    /// Check if this is a complete value.
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete(_))
    }

    /// Check if this is a partial value.
    pub fn is_partial(&self) -> bool {
        matches!(self, Self::Partial(_))
    }

    /// Check if this is missing.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// Get the complete value if present.
    pub fn as_complete(&self) -> Option<&T> {
        match self {
            Self::Complete(v) => Some(v),
            _ => None,
        }
    }

    /// Get the partial value if present.
    pub fn as_partial(&self) -> Option<&JSONValue> {
        match self {
            Self::Partial(v) => Some(v),
            _ => None,
        }
    }

    /// Convert to a complete value or default.
    pub fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        match self {
            Self::Complete(v) => v,
            _ => T::default(),
        }
    }

    /// Map the complete value.
    pub fn map<U, F>(self, f: F) -> DeepPartial<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            Self::Complete(v) => DeepPartial::complete(f(v)),
            Self::Partial(v) => DeepPartial::partial(v),
            Self::Missing => DeepPartial::missing(),
        }
    }
}

/// Merge two partial JSON values.
///
/// # Arguments
///
/// * `base` - The base value.
/// * `update` - The update value.
///
/// # Returns
///
/// The merged value.
pub fn merge_partial_json(base: &JSONValue, update: &JSONValue) -> JSONValue {
    match (base, update) {
        (JSONValue::Object(base_obj), JSONValue::Object(update_obj)) => {
            let mut result = base_obj.clone();
            for (key, value) in update_obj {
                if let Some(base_value) = result.get(key) {
                    result.insert(key.clone(), merge_partial_json(base_value, value));
                } else {
                    result.insert(key.clone(), value.clone());
                }
            }
            JSONValue::Object(result)
        }
        (JSONValue::Array(base_arr), JSONValue::Array(update_arr)) => {
            let mut result = base_arr.clone();
            for (i, value) in update_arr.iter().enumerate() {
                if i < result.len() {
                    result[i] = merge_partial_json(&result[i], value);
                } else {
                    result.push(value.clone());
                }
            }
            JSONValue::Array(result)
        }
        // For non-object/array types, update wins
        (_, update) => update.clone(),
    }
}

/// Check if a JSON value is a partial (has null or missing required fields).
///
/// # Arguments
///
/// * `value` - The value to check.
/// * `required_fields` - The required field names.
///
/// # Returns
///
/// True if any required field is missing or null.
pub fn is_partial_object(value: &JSONValue, required_fields: &[&str]) -> bool {
    match value {
        JSONValue::Object(obj) => {
            for field in required_fields {
                match obj.get(*field) {
                    None => return true,
                    Some(JSONValue::Null) => return true,
                    _ => continue,
                }
            }
            false
        }
        _ => true,
    }
}
