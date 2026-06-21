//! JSON Schema derivation utilities.
//!
//! This module provides utilities for generating JSON schemas from Rust types.
//! It uses the `schemars` crate when the `json-schema` feature is enabled.

#[cfg(feature = "json-schema")]
use schemars::JsonSchema;
#[cfg(feature = "json-schema")]
use schemars::schema_for;
use serde_json::Value as JSONValue;

/// A generated JSON schema.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedSchema {
    /// The schema as a JSON value.
    pub schema: JSONValue,
}

impl GeneratedSchema {
    /// Create a new generated schema.
    pub fn new(schema: JSONValue) -> Self {
        Self { schema }
    }

    /// Get the schema as a JSON value.
    pub fn as_json(&self) -> &JSONValue {
        &self.schema
    }

    /// Convert to a pretty-printed JSON string.
    pub fn to_string_pretty(&self) -> String {
        serde_json::to_string_pretty(&self.schema).unwrap_or_default()
    }
}

/// Generate a JSON schema from a type.
///
/// This function is only available when the `json-schema` feature is enabled.
///
/// # Example
///
/// ```ignore
/// use vercel_ai_provider_utils::schema_from_type;
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(JsonSchema, Deserialize)]
/// struct User {
///     name: String,
///     age: u32,
/// }
///
/// let schema = schema_from_type::<User>();
/// println!("{}", schema.to_string_pretty());
/// ```
#[cfg(feature = "json-schema")]
pub fn schema_from_type<T: JsonSchema>() -> GeneratedSchema {
    let schema = schema_for!(T);
    let json = serde_json::to_value(&schema).unwrap_or(JSONValue::Null);
    GeneratedSchema::new(json)
}

/// Generate a JSON schema from a type (stub for when json-schema feature is disabled).
///
/// Returns an empty object when the feature is not enabled.
#[cfg(not(feature = "json-schema"))]
pub fn schema_from_type<T>() -> GeneratedSchema {
    GeneratedSchema::new(JSONValue::Object(serde_json::Map::new()))
}

/// Generate a JSON schema for a type and return it as a JSON value.
///
/// This is a convenience function that calls [`schema_from_type`] and returns
/// the inner JSON value.
#[cfg(feature = "json-schema")]
pub fn json_schema_from_type<T: JsonSchema>() -> JSONValue {
    schema_from_type::<T>().schema
}

/// Generate a JSON schema for a type (stub for when json-schema feature is disabled).
#[cfg(not(feature = "json-schema"))]
pub fn json_schema_from_type<T>() -> JSONValue {
    JSONValue::Object(serde_json::Map::new())
}

/// Merge additional properties into a schema.
///
/// This is useful for adding custom properties to a generated schema.
pub fn merge_into_schema(schema: &mut GeneratedSchema, key: &str, value: JSONValue) {
    if let JSONValue::Object(ref mut map) = schema.schema {
        map.insert(key.to_string(), value);
    }
}

/// Add a "required" field to the schema.
///
/// Marks the given fields as required in the schema.
pub fn add_required_fields(schema: &mut GeneratedSchema, fields: &[&str]) {
    if let JSONValue::Object(ref mut map) = schema.schema {
        let required: JSONValue = fields
            .iter()
            .map(|s| JSONValue::String(s.to_string()))
            .collect();
        map.insert("required".to_string(), required);
    }
}

#[cfg(test)]
#[path = "json_schema_derive.test.rs"]
mod tests;
