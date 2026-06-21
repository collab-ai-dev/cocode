//! Inject JSON instruction into prompts for structured output.
//!
//! This module provides utilities for injecting instructions that help
//! models generate valid JSON output.

use serde_json::Value;

/// Inject JSON format instruction into a prompt.
///
/// This prepends instructions to the prompt that help ensure the model
/// generates valid JSON output matching the provided schema.
///
/// # Arguments
///
/// * `prompt` - The original prompt
/// * `schema` - The JSON schema the output should match
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::inject_json_instruction;
/// use serde_json::json;
///
/// let prompt = "Extract the user's name and age.";
/// let schema = json!({
///     "type": "object",
///     "properties": {
///         "name": { "type": "string" },
///         "age": { "type": "integer" }
///     }
/// });
///
/// let result = inject_json_instruction(prompt, &schema);
/// assert!(result.contains("JSON"));
/// assert!(result.contains("\"type\": \"object\""));
/// ```
pub fn inject_json_instruction(prompt: &str, schema: &Value) -> String {
    format!(
        "{}\n\nRespond with a JSON object that matches the following schema:\n{}\n\nDo not include any text outside the JSON object.",
        prompt,
        serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
    )
}

/// Inject JSON format instruction with a custom description.
///
/// This allows customizing the instruction text while still including
/// the schema.
///
/// # Arguments
///
/// * `prompt` - The original prompt
/// * `schema` - The JSON schema the output should match
/// * `description` - Custom description for the expected output format
pub fn inject_json_instruction_with_description(
    prompt: &str,
    schema: &Value,
    description: &str,
) -> String {
    format!(
        "{}\n\n{}\n\nSchema:\n{}",
        prompt,
        description,
        serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
    )
}

/// Create a JSON response format instruction.
///
/// Returns a formatted instruction string for JSON output without
/// modifying the original prompt.
pub fn create_json_response_instruction(schema: &Value) -> String {
    format!(
        "Respond with a JSON object matching this schema:\n{}\n\nOutput only valid JSON, with no additional text or formatting.",
        serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
    )
}

/// Inject JSON instruction for array output.
///
/// Specialized instruction for when the expected output is a JSON array.
pub fn inject_json_array_instruction(prompt: &str, item_schema: &Value) -> String {
    format!(
        "{}\n\nRespond with a JSON array where each item matches the following schema:\n{}\n\nOutput only the JSON array, with no additional text.",
        prompt,
        serde_json::to_string_pretty(item_schema).unwrap_or_else(|_| item_schema.to_string())
    )
}

#[cfg(test)]
#[path = "inject_json_instruction.test.rs"]
mod tests;
