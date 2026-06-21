//! Error message extraction utilities.

use serde_json::Value;

/// Extract an error message from a JSON response.
///
/// Checks common error message fields in order:
/// 1. error.message
/// 2. error
/// 3. message
/// 4. detail
/// 5. error_description
pub fn get_error_message(json: &Value) -> String {
    // Try error.message
    if let Some(msg) = json
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return msg.to_string();
    }

    // Try error as string
    if let Some(msg) = json.get("error").and_then(|e| e.as_str()) {
        return msg.to_string();
    }

    // Try message
    if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
        return msg.to_string();
    }

    // Try detail
    if let Some(msg) = json.get("detail").and_then(|d| d.as_str()) {
        return msg.to_string();
    }

    // Try error_description
    if let Some(msg) = json.get("error_description").and_then(|e| e.as_str()) {
        return msg.to_string();
    }

    // Fallback to the whole JSON
    json.to_string()
}

/// Extract an error code from a JSON response.
pub fn get_error_code(json: &Value) -> Option<String> {
    // Try error.code
    if let Some(code) = json
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
    {
        return Some(code.to_string());
    }

    // Try code
    if let Some(code) = json.get("code").and_then(|c| c.as_str()) {
        return Some(code.to_string());
    }

    // Try type
    if let Some(t) = json.get("type").and_then(|t| t.as_str()) {
        return Some(t.to_string());
    }

    None
}

#[cfg(test)]
#[path = "error_message.test.rs"]
mod tests;
