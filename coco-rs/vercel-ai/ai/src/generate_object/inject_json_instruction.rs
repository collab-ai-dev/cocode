//! Inject JSON format instructions.
//!
//! This module provides utilities for injecting JSON format instructions
//! into prompts for structured output generation.

use serde_json::Value;

const DEFAULT_SCHEMA_PREFIX: &str = "JSON schema:";
const DEFAULT_SCHEMA_SUFFIX: &str =
    "You MUST answer with a JSON object that matches the JSON schema above.";
const DEFAULT_GENERIC_SUFFIX: &str = "You MUST answer with JSON.";

/// Inject JSON format instructions into a prompt.
///
/// This function adds schema information and formatting instructions
/// to help the model generate valid JSON output.
pub fn inject_json_instruction(prompt: Option<&str>, schema: Option<&Value>) -> String {
    inject_json_instruction_with_options(
        prompt,
        schema,
        schema.map(|_| DEFAULT_SCHEMA_PREFIX),
        schema
            .map(|_| DEFAULT_SCHEMA_SUFFIX)
            .unwrap_or(DEFAULT_GENERIC_SUFFIX),
    )
}

/// Inject JSON format instructions with custom prefix and suffix.
pub fn inject_json_instruction_with_options(
    prompt: Option<&str>,
    schema: Option<&Value>,
    schema_prefix: Option<&str>,
    schema_suffix: &str,
) -> String {
    let mut parts = Vec::new();

    // Add prompt if present
    if let Some(p) = prompt
        && !p.is_empty()
    {
        parts.push(p.to_string());
        parts.push(String::new()); // Add empty line after prompt
    }

    // Add schema prefix
    if let Some(prefix) = schema_prefix {
        parts.push(prefix.to_string());
    }

    // Add schema
    if let Some(s) = schema {
        parts.push(serde_json::to_string_pretty(s).unwrap_or_default());
    }

    // Add schema suffix
    parts.push(schema_suffix.to_string());

    parts
        .into_iter()
        .filter(|s| !s.is_empty() || prompt.is_some())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "inject_json_instruction.test.rs"]
mod tests;
