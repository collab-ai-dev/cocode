//! Parse partial JSON from streaming responses.
//!
//! This module provides utilities for parsing incomplete JSON that may
/// be received during streaming responses.
use serde_json::Value;

/// Parse partial JSON, attempting to complete incomplete structures.
///
/// This function tries to parse JSON that may be incomplete (e.g., during
/// streaming). It will attempt to close open braces and brackets.
///
/// # Arguments
///
/// * `text` - The partial JSON text to parse.
///
/// # Returns
///
/// `Some(Value)` if parsing succeeded, `None` otherwise.
pub fn parse_partial_json(text: &str) -> Option<Value> {
    // Try to parse as-is first
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }

    // Try to complete partial JSON
    let completed = complete_partial_json(text);
    serde_json::from_str(&completed).ok()
}

/// Complete partial JSON by closing open structures.
///
/// # Arguments
///
/// * `text` - The partial JSON text.
///
/// # Returns
///
/// A completed JSON string with closed braces/brackets.
pub fn complete_partial_json(text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();

    // Count open braces/brackets
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

    // If we're in a string, close it
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

    chars.into_iter().collect()
}

/// Parse partial JSON with repair for common issues.
///
/// This function attempts to parse partial JSON and also handles
/// common issues like trailing commas.
///
/// # Arguments
///
/// * `text` - The partial JSON text.
///
/// # Returns
///
/// `Some(Value)` if parsing succeeded after repairs, `None` otherwise.
pub fn parse_partial_json_with_repair(text: &str) -> Option<Value> {
    // Try direct parse first
    if let Some(v) = parse_partial_json(text) {
        return Some(v);
    }

    // Try removing trailing commas
    let repaired = remove_trailing_commas(text);
    if let Some(v) = parse_partial_json(&repaired) {
        return Some(v);
    }

    None
}

/// Remove trailing commas from JSON text.
///
/// # Arguments
///
/// * `text` - The JSON text.
///
/// # Returns
///
/// JSON text with trailing commas removed.
fn remove_trailing_commas(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut in_string = false;
    let mut escape_next = false;

    for i in 0..chars.len() {
        let c = chars[i];

        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        match c {
            '\\' if in_string => {
                result.push(c);
                escape_next = true;
            }
            '"' => {
                result.push(c);
                in_string = !in_string;
            }
            ',' if !in_string => {
                // Check if followed by } or ]
                let next_non_ws = chars[i + 1..]
                    .iter()
                    .find(|&&c| !c.is_whitespace())
                    .copied();

                if matches!(next_non_ws, Some('}') | Some(']')) {
                    // Skip trailing comma
                } else {
                    result.push(c);
                }
            }
            _ => {
                result.push(c);
            }
        }
    }

    result
}

/// Extract a value from partial JSON at a given path.
///
/// # Arguments
///
/// * `text` - The partial JSON text.
/// * `path` - The path segments to extract.
///
/// # Returns
///
/// The value at the path, if found.
pub fn extract_partial_value(text: &str, path: &[&str]) -> Option<Value> {
    let json = parse_partial_json(text)?;
    let mut current = &json;

    for segment in path {
        match current {
            Value::Object(map) => {
                current = map.get(*segment)?;
            }
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

#[cfg(test)]
#[path = "parse_partial_json.test.rs"]
mod tests;
