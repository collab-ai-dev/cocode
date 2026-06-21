//! Schema utilities for structured output.

use serde::de::DeserializeOwned;
use serde_json::Value;
use vercel_ai_provider::JSONSchema;

/// Trait for schema validation.
pub trait Schema: Send + Sync {
    /// The output type of this schema.
    type Output: DeserializeOwned;

    /// Get the JSON schema.
    fn json_schema(&self) -> &JSONSchema;

    /// Validate a value against the schema.
    fn validate(&self, value: &Value) -> Result<Self::Output, ValidationError>;
}

/// A simple JSON schema wrapper.
pub struct JsonSchemaWrapper<T> {
    schema: JSONSchema,
    #[cfg(feature = "schema-validation")]
    compiled: Option<jsonschema::Validator>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> JsonSchemaWrapper<T> {
    /// Create a new schema wrapper.
    pub fn new(schema: JSONSchema) -> Self {
        #[cfg(feature = "schema-validation")]
        {
            let compiled = jsonschema::validator_for(&schema).ok();
            Self {
                schema,
                compiled,
                _marker: std::marker::PhantomData,
            }
        }
        #[cfg(not(feature = "schema-validation"))]
        Self {
            schema,
            _marker: std::marker::PhantomData,
        }
    }

    /// Create a new schema wrapper without compilation (for empty schemas).
    fn empty() -> Self {
        Self::new(Value::Object(serde_json::Map::new()))
    }
}

impl<T: DeserializeOwned + Send + Sync + 'static> Schema for JsonSchemaWrapper<T> {
    type Output = T;

    fn json_schema(&self) -> &JSONSchema {
        &self.schema
    }

    fn validate(&self, value: &Value) -> Result<Self::Output, ValidationError> {
        #[cfg(feature = "schema-validation")]
        {
            if let Some(compiled) = &self.compiled {
                let messages: Vec<String> = compiled
                    .iter_errors(value)
                    .map(|e| format!("{} at {}", e, e.instance_path()))
                    .collect();
                if !messages.is_empty() {
                    return Err(ValidationError::SchemaValidation(messages));
                }
            }
        }

        // Deserialize the value
        serde_json::from_value(value.clone())
            .map_err(|e| ValidationError::ParseError(e.to_string()))
    }
}

/// Create a schema from a JSON schema.
pub fn json_schema<T: DeserializeOwned + Send + Sync + 'static>(
    schema: JSONSchema,
) -> impl Schema<Output = T> {
    JsonSchemaWrapper::<T>::new(schema)
}

/// Create a schema from a type (uses serde for validation).
pub fn as_schema<T: DeserializeOwned + Send + Sync + 'static>() -> impl Schema<Output = T> {
    JsonSchemaWrapper::<T>::empty()
}

/// Validation error.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// JSON Schema validation errors
    #[error("Schema validation errors: {}", .0.join("; "))]
    SchemaValidation(Vec<String>),

    /// JSON parse error during deserialization
    #[error("Parse error: {0}")]
    ParseError(String),
}

impl ValidationError {
    /// Create a validation error with a single message.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::SchemaValidation(vec![message.into()])
    }
}

#[cfg(test)]
#[path = "schema.test.rs"]
mod tests;
