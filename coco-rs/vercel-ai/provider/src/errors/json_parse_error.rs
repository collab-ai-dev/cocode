//! JSON parse error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when JSON parsing fails.
#[derive(Debug, Error)]
pub struct JSONParseError {
    /// The text that failed to parse.
    pub text: String,
    /// The error message.
    pub message: String,
    /// The underlying cause.
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for JSONParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl JSONParseError {
    /// Create a new JSON parse error.
    pub fn new(text: impl Into<String>, cause: Box<dyn std::error::Error + Send + Sync>) -> Self {
        let text = text.into();
        let message = format!(
            "JSON parsing failed: Text: {}.\nError message: {}",
            &text, cause
        );
        Self {
            text,
            message,
            cause: Some(cause),
        }
    }
}

impl From<serde_json::Error> for JSONParseError {
    fn from(err: serde_json::Error) -> Self {
        let message = format!("JSON parsing failed: {err}");
        Self {
            text: String::new(),
            message,
            cause: Some(Box::new(err)),
        }
    }
}

#[cfg(test)]
#[path = "json_parse_error.test.rs"]
mod tests;
