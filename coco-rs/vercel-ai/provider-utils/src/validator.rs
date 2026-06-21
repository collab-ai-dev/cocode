//! Input validation utilities.
//!
//! This module provides validation functions for common inputs.

use regex::Regex;
use std::sync::LazyLock;

/// Regex for valid tool names: alphanumeric, underscore, hyphen, 1-64 chars.
static TOOL_NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)]
    Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").unwrap()
});

/// Regex for valid model IDs: alphanumeric, underscore, hyphen, dot, colon, slash, 1-128 chars.
static MODEL_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)]
    Regex::new(r"^[a-zA-Z0-9_.:/-]{1,128}$").unwrap()
});

/// Validation error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Validation error for '{}': {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

impl ValidationError {
    /// Create a new validation error.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// Validate a tool name.
///
/// Tool names must:
/// - Be 1-64 characters long
/// - Contain only alphanumeric characters, underscores, and hyphens
///
/// # Arguments
///
/// * `name` - The tool name to validate.
///
/// # Returns
///
/// `Ok(())` if valid, `Err(ValidationError)` otherwise.
pub fn validate_tool_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::new(
            "tool_name",
            "Tool name cannot be empty",
        ));
    }

    if name.len() > 64 {
        return Err(ValidationError::new(
            "tool_name",
            "Tool name cannot exceed 64 characters",
        ));
    }

    if !TOOL_NAME_REGEX.is_match(name) {
        return Err(ValidationError::new(
            "tool_name",
            "Tool name must contain only alphanumeric characters, underscores, and hyphens",
        ));
    }

    Ok(())
}

/// Validate a model ID.
///
/// Model IDs must:
/// - Be 1-128 characters long
/// - Contain only alphanumeric characters, underscores, hyphens, dots, colons, and slashes
///
/// # Arguments
///
/// * `id` - The model ID to validate.
///
/// # Returns
///
/// `Ok(())` if valid, `Err(ValidationError)` otherwise.
pub fn validate_model_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty() {
        return Err(ValidationError::new("model_id", "Model ID cannot be empty"));
    }

    if id.len() > 128 {
        return Err(ValidationError::new(
            "model_id",
            "Model ID cannot exceed 128 characters",
        ));
    }

    if !MODEL_ID_REGEX.is_match(id) {
        return Err(ValidationError::new(
            "model_id",
            "Model ID must contain only alphanumeric characters, underscores, hyphens, dots, colons, and slashes",
        ));
    }

    Ok(())
}

/// Validate a URL.
///
/// # Arguments
///
/// * `url` - The URL to validate.
///
/// # Returns
///
/// `Ok(())` if valid, `Err(ValidationError)` otherwise.
pub fn validate_url(url: &str) -> Result<(), ValidationError> {
    if url.is_empty() {
        return Err(ValidationError::new("url", "URL cannot be empty"));
    }

    match url::Url::parse(url) {
        Ok(parsed) => {
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(ValidationError::new(
                    "url",
                    "URL must use http or https scheme",
                ));
            }
            Ok(())
        }
        Err(e) => Err(ValidationError::new("url", format!("Invalid URL: {e}"))),
    }
}

/// Validate an API key format.
///
/// API keys must:
/// - Not be empty
/// - Not contain whitespace
/// - Not be a placeholder value like "your-api-key" or "sk-xxx"
///
/// # Arguments
///
/// * `key` - The API key to validate.
///
/// # Returns
///
/// `Ok(())` if valid, `Err(ValidationError)` otherwise.
pub fn validate_api_key(key: &str) -> Result<(), ValidationError> {
    if key.is_empty() {
        return Err(ValidationError::new("api_key", "API key cannot be empty"));
    }

    if key.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(
            "api_key",
            "API key cannot contain whitespace",
        ));
    }

    // Check for common placeholder values
    let lower = key.to_lowercase();
    let placeholders = [
        "your-api-key",
        "your_api_key",
        "api_key",
        "apikey",
        "sk-xxx",
        "xxx",
        "placeholder",
        "test",
    ];

    for placeholder in placeholders {
        if lower == placeholder {
            return Err(ValidationError::new(
                "api_key",
                "API key appears to be a placeholder value",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "validator.test.rs"]
mod tests;
