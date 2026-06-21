//! Parse and validate object results.
//!
//! This module provides utilities for parsing and validating
//! JSON output from language models.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::AIError;

/// Result of parsing and validating an object.
#[derive(Debug)]
pub struct ParsedObjectResult<T> {
    /// The parsed object.
    pub object: T,
    /// The raw JSON string.
    pub raw: String,
    /// Whether the result was repaired.
    pub was_repaired: bool,
}

/// Parse and validate a JSON string into a typed object.
///
/// This function attempts to parse the JSON string and validate it
/// against the expected type. If parsing fails, it attempts repairs.
pub fn parse_and_validate<T: DeserializeOwned>(
    text: &str,
) -> Result<ParsedObjectResult<T>, AIError> {
    // Try direct parsing first
    match serde_json::from_str::<T>(text) {
        Ok(object) => Ok(ParsedObjectResult {
            object,
            raw: text.to_string(),
            was_repaired: false,
        }),
        Err(e) => {
            // Try to repair and parse
            let repaired = super::repair_text::repair_json_text(text);

            if let Some(fixed) = repaired {
                match serde_json::from_str::<T>(&fixed) {
                    Ok(object) => {
                        return Ok(ParsedObjectResult {
                            object,
                            raw: fixed,
                            was_repaired: true,
                        });
                    }
                    Err(_) => {
                        // Repair didn't help, return original error
                    }
                }
            }

            Err(AIError::SchemaValidation(format!(
                "Failed to parse JSON: {e}"
            )))
        }
    }
}

/// Parse JSON text into a generic Value.
pub fn parse_json_value(text: &str) -> Result<Value, AIError> {
    // Try direct parsing
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }

    // Try repair
    if let Some(fixed) = super::repair_text::repair_json_text(text)
        && let Ok(value) = serde_json::from_str(&fixed)
    {
        return Ok(value);
    }

    Err(AIError::SchemaValidation(
        "Failed to parse JSON".to_string(),
    ))
}

/// Validate that a value matches a JSON schema.
pub fn validate_against_schema(value: &Value, schema: &Value) -> Result<(), AIError> {
    // Basic schema validation
    // This is a simplified implementation - a full implementation would use
    // a JSON schema validation library

    if let Some(schema_type) = schema.get("type")
        && let Some(type_str) = schema_type.as_str()
    {
        let value_type = json_type(value);
        if !type_matches(value_type, type_str) {
            return Err(AIError::SchemaValidation(format!(
                "Expected type '{type_str}', got '{value_type}'"
            )));
        }
    }

    Ok(())
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(actual: &str, expected: &str) -> bool {
    if expected == "integer" {
        return actual == "number"; // Simplified
    }
    actual == expected
}

#[cfg(test)]
#[path = "parse_validate.test.rs"]
mod tests;
