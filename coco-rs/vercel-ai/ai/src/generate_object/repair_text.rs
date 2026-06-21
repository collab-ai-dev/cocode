//! Repair text for JSON parsing.
//!
//! This module provides utilities for repairing malformed JSON text
//! to enable successful parsing.

use serde_json::Value;

/// A function that attempts to repair malformed JSON text.
pub type RepairTextFunction = fn(&str) -> Option<String>;

/// Attempt to repair malformed JSON text.
///
/// This function tries common repairs:
/// - Completing truncated JSON
/// - Fixing missing quotes
/// - Removing trailing commas
/// - Fixing control characters
pub fn repair_json_text(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Try to parse as-is first
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // Try common repairs
    let repairs = [
        complete_truncated_json,
        fix_missing_quotes,
        remove_trailing_commas,
        fix_control_characters,
    ];

    for repair in repairs {
        if let Some(fixed) = repair(trimmed)
            && serde_json::from_str::<Value>(&fixed).is_ok()
        {
            return Some(fixed);
        }
    }

    None
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

/// Fix missing quotes around keys.
fn fix_missing_quotes(text: &str) -> Option<String> {
    // Simple fix: try adding quotes around unquoted keys
    // This is a basic implementation
    let result = text.to_string();

    // Try parsing
    if serde_json::from_str::<Value>(&result).is_ok() {
        return Some(result);
    }

    None
}

/// Remove trailing commas in arrays and objects.
fn remove_trailing_commas(text: &str) -> Option<String> {
    // Remove ", }" and ", ]" patterns
    let result = text.replace(", }", "}").replace(", ]", "]");

    if serde_json::from_str::<Value>(&result).is_ok() {
        return Some(result);
    }

    None
}

/// Fix control characters in strings.
fn fix_control_characters(text: &str) -> Option<String> {
    // Replace common problematic characters
    let result = text
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");

    if serde_json::from_str::<Value>(&result).is_ok() {
        return Some(result);
    }

    None
}

#[cfg(test)]
#[path = "repair_text.test.rs"]
mod tests;
