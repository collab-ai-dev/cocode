//! No content generated error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when the AI provider fails to generate any content.
#[derive(Debug, Error)]
pub struct NoContentGeneratedError {
    /// The error message.
    pub message: String,
}

impl fmt::Display for NoContentGeneratedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl NoContentGeneratedError {
    /// Create a new no content generated error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Default for NoContentGeneratedError {
    fn default() -> Self {
        Self {
            message: "No content generated.".to_string(),
        }
    }
}

#[cfg(test)]
#[path = "no_content_generated_error.test.rs"]
mod tests;
