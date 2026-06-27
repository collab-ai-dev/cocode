//! Validate object generation input.
//!
//! This module provides validation utilities for object generation inputs.

use vercel_ai_provider::JSONSchema;

use crate::error::AIError;

use super::ObjectGenerationMode;

/// Validate object generation input.
///
/// # Arguments
///
/// * `schema` - The JSON schema for the output.
/// * `mode` - The generation mode.
///
/// # Returns
///
/// Ok if valid, Err with validation error otherwise.
pub fn validate_object_generation_input(
    schema: &JSONSchema,
    mode: ObjectGenerationMode,
) -> Result<(), AIError> {
    // Validate that the schema is an object type for certain modes
    match mode {
        ObjectGenerationMode::Auto | ObjectGenerationMode::Json => {
            // These modes are flexible - any schema is acceptable
            Ok(())
        }
        ObjectGenerationMode::Tool => {
            // Tool mode works best with object schemas
            validate_object_schema(schema)
        }
        ObjectGenerationMode::Grammar => {
            // Grammar mode requires specific schema structure
            validate_grammar_schema(schema)
        }
    }
}

/// Validate that the schema is suitable for object generation.
fn validate_object_schema(schema: &JSONSchema) -> Result<(), AIError> {
    // Check if it's an object type
    if let Some(schema_type) = schema.get("type").and_then(|t| t.as_str())
        && schema_type != "object"
    {
        // Not an error, just a warning in practice
        // Some models can handle non-object schemas
    }

    // Check for required properties
    if let Some(props) = schema.get("properties")
        && !props.is_object()
    {
        return Err(AIError::InvalidArgument(
            "Schema properties must be an object".to_string(),
        ));
    }

    Ok(())
}

/// Validate that the schema is suitable for grammar-constrained generation.
fn validate_grammar_schema(schema: &JSONSchema) -> Result<(), AIError> {
    // Grammar mode typically requires a more restrictive schema
    // For now, just check that it's a valid schema
    if schema.is_null() {
        return Err(AIError::InvalidArgument(
            "Schema cannot be null for grammar mode".to_string(),
        ));
    }

    Ok(())
}

/// Determine the best generation mode based on model capabilities.
///
/// # Arguments
///
/// * `supports_json` - Whether the model supports JSON mode.
/// * `supports_tools` - Whether the model supports tool calling.
/// * `supports_grammar` - Whether the model supports grammar-constrained generation.
///
/// # Returns
///
/// The best generation mode to use.
pub fn determine_generation_mode(
    supports_json: bool,
    supports_tools: bool,
    supports_grammar: bool,
) -> ObjectGenerationMode {
    // Prefer tool mode as it provides the best structured output
    if supports_tools {
        return ObjectGenerationMode::Tool;
    }

    // Fall back to JSON mode
    if supports_json {
        return ObjectGenerationMode::Json;
    }

    // Last resort: grammar mode
    if supports_grammar {
        return ObjectGenerationMode::Grammar;
    }

    // Default to auto
    ObjectGenerationMode::Auto
}
