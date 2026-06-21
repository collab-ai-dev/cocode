//! Unsupported functionality error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when a functionality is not supported by a provider or model.
#[derive(Debug, Error)]
pub struct UnsupportedFunctionalityError {
    /// The error message.
    pub message: String,
    /// The functionality that is not supported.
    pub functionality: String,
}

impl fmt::Display for UnsupportedFunctionalityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unsupported functionality: {}", self.message)
    }
}

impl UnsupportedFunctionalityError {
    /// Create a new unsupported functionality error.
    pub fn new(functionality: impl Into<String>) -> Self {
        let functionality = functionality.into();
        Self {
            message: format!("'{functionality}' is not supported by this provider or model"),
            functionality,
        }
    }

    /// Create an error with a custom message.
    pub fn with_message(functionality: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            functionality: functionality.into(),
        }
    }
}

#[cfg(test)]
#[path = "unsupported_functionality_error.test.rs"]
mod tests;
