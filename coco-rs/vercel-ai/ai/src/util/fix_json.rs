//! Fix malformed JSON.
//!
//! This module provides utilities for fixing common JSON formatting issues.

use serde_json::Value;

/// Attempt to fix malformed JSON text.
///
/// This function tries several common fixes:
/// - Completing truncated JSON
/// - Removing trailing commas
/// - Fixing control characters
/// - Removing markdown code blocks
pub fn fix_json(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Try to parse as-is first
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // Remove markdown code blocks
    let without_markdown = remove_markdown_blocks(trimmed);
    if serde_json::from_str::<Value>(&without_markdown).is_ok() {
        return Some(without_markdown);
    }

    // Try completing truncated JSON
    if let Some(completed) = complete_truncated_json(&without_markdown)
        && serde_json::from_str::<Value>(&completed).is_ok()
    {
        return Some(completed);
    }

    // Try removing trailing commas
    let without_trailing = remove_trailing_commas(&without_markdown);
    if serde_json::from_str::<Value>(&without_trailing).is_ok() {
        return Some(without_trailing);
    }

    None
}

/// Remove markdown code blocks from text.
fn remove_markdown_blocks(text: &str) -> String {
    let mut result = text.to_string();

    // Remove ```json and ``` markers
    result = result.replace("```json", "");
    result = result.replace("```JSON", "");
    result = result.replace("```", "");

    result.trim().to_string()
}

/// Complete truncated JSON by adding missing closing brackets.
fn complete_truncated_json(text: &str) -> Option<String> {
    let mut chars: Vec<char> = text.chars().collect();

    let mut open_braces = 0i32;
    let mut open_brackets = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for &c in &chars {
        if escape_next {
            escape_next = false;
            continue;
        }
        match c {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => open_braces += 1,
            '}' if !in_string => open_braces -= 1,
            '[' if !in_string => open_brackets += 1,
            ']' if !in_string => open_brackets -= 1,
            _ => {}
        }
    }

    // Close unclosed string
    if in_string {
        chars.push('"');
    }

    // Close open structures
    #[allow(clippy::same_item_push)]
    for _ in 0..open_brackets {
        chars.push(']');
    }
    #[allow(clippy::same_item_push)]
    for _ in 0..open_braces {
        chars.push('}');
    }

    Some(chars.into_iter().collect())
}

/// Remove trailing commas in arrays and objects.
fn remove_trailing_commas(text: &str) -> String {
    text.replace(", }", "}")
        .replace(", ]", "]")
        .replace(",}", "}")
        .replace(",]", "]")
}

/// Check if text is valid JSON.
pub fn is_valid_json(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok()
}

#[cfg(test)]
#[path = "fix_json.test.rs"]
mod tests;
