//! JSON Schema type alias.
//!
//! JSON Schema is represented as a JSON Value in this SDK.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// A JSON Schema definition.
///
/// This is represented as a JSON Value, allowing for any valid JSON Schema.
pub type JSONSchema = Value;

/// A JSON Schema with additional metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONSchemaDefinition {
    /// The JSON Schema.
    #[serde(flatten)]
    pub schema: JSONSchema,
    /// An optional name for the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// An optional description for the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl JSONSchemaDefinition {
    /// Create a new JSON schema definition from a schema value.
    pub fn new(schema: JSONSchema) -> Self {
        Self {
            schema,
            name: None,
            description: None,
        }
    }

    /// Add a name to the schema.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add a description to the schema.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl From<JSONSchema> for JSONSchemaDefinition {
    fn from(schema: JSONSchema) -> Self {
        Self::new(schema)
    }
}

/// Helper to create a simple object JSON schema.
pub fn object_schema(properties: HashMap<String, JSONSchema>, required: Vec<String>) -> JSONSchema {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

/// Helper to create a simple string JSON schema.
pub fn string_schema() -> JSONSchema {
    serde_json::json!({ "type": "string" })
}

/// Helper to create a simple number JSON schema.
pub fn number_schema() -> JSONSchema {
    serde_json::json!({ "type": "number" })
}

/// Helper to create a simple boolean JSON schema.
pub fn boolean_schema() -> JSONSchema {
    serde_json::json!({ "type": "boolean" })
}

/// Helper to create an array JSON schema.
pub fn array_schema(items: JSONSchema) -> JSONSchema {
    serde_json::json!({
        "type": "array",
        "items": items
    })
}
