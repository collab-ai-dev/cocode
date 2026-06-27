//! Output strategy for generate_object.
//!
//! This module provides types for configuring how structured output
//! should be generated.

use vercel_ai_provider::JSONSchema;
use vercel_ai_provider::ResponseFormat;

/// Strategy for generating structured output.
#[derive(Debug, Clone, Default)]
pub enum ObjectOutputStrategy {
    /// Generate a JSON object matching the schema.
    Object {
        /// The JSON schema for the object.
        schema: JSONSchema,
    },
    /// Generate a JSON array of objects matching the schema.
    Array {
        /// The JSON schema for array items.
        schema: JSONSchema,
    },
    /// Generate one of a set of enum values.
    Enum {
        /// The allowed enum values.
        values: Vec<String>,
    },
    /// No schema - just request JSON output.
    #[default]
    NoSchema,
}

impl ObjectOutputStrategy {
    /// Create an object output strategy.
    pub fn object(schema: JSONSchema) -> Self {
        Self::Object { schema }
    }

    /// Create an array output strategy.
    pub fn array(schema: JSONSchema) -> Self {
        Self::Array { schema }
    }

    /// Create an enum output strategy.
    pub fn enum_values(values: Vec<String>) -> Self {
        Self::Enum { values }
    }

    /// Create a no-schema strategy.
    pub fn no_schema() -> Self {
        Self::NoSchema
    }

    /// Get the JSON schema, if any.
    pub fn schema(&self) -> Option<&JSONSchema> {
        match self {
            Self::Object { schema } => Some(schema),
            Self::Array { schema } => Some(schema),
            Self::Enum { .. } => None,
            Self::NoSchema => None,
        }
    }

    /// Convert to a ResponseFormat for the provider.
    pub fn to_response_format(&self, name: Option<&str>) -> Option<ResponseFormat> {
        match self {
            Self::Object { schema } => {
                let format_name = name.unwrap_or("object");
                Some(
                    ResponseFormat::json_with_schema(schema.clone())
                        .with_name(format_name.to_string()),
                )
            }
            Self::Array { schema } => {
                // Wrap schema in array schema
                let array_schema = serde_json::json!({
                    "type": "array",
                    "items": schema
                });
                let format_name = name.unwrap_or("array");
                Some(
                    ResponseFormat::json_with_schema(array_schema)
                        .with_name(format_name.to_string()),
                )
            }
            Self::Enum { values } => {
                // Create enum schema
                let enum_schema = serde_json::json!({
                    "type": "string",
                    "enum": values
                });
                let format_name = name.unwrap_or("enum");
                Some(
                    ResponseFormat::json_with_schema(enum_schema)
                        .with_name(format_name.to_string()),
                )
            }
            Self::NoSchema => Some(ResponseFormat::json()),
        }
    }
}

/// Trait for types that can provide an output strategy.
///
/// This trait allows ergonomic conversion from schemas to output strategies.
#[allow(dead_code)]
pub trait IntoOutputStrategy {
    /// Convert into an output strategy.
    fn into_output_strategy(self) -> ObjectOutputStrategy;
}

impl IntoOutputStrategy for JSONSchema {
    fn into_output_strategy(self) -> ObjectOutputStrategy {
        ObjectOutputStrategy::object(self)
    }
}

impl IntoOutputStrategy for ObjectOutputStrategy {
    fn into_output_strategy(self) -> ObjectOutputStrategy {
        self
    }
}

#[cfg(test)]
#[path = "output_strategy.test.rs"]
mod tests;
