//! Invalid argument error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when a function argument is invalid.
#[derive(Debug, Error)]
pub struct InvalidArgumentError {
    /// The name of the invalid argument.
    pub argument: String,
    /// The error message.
    pub message: String,
    /// The underlying cause (if any).
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for InvalidArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid argument '{}': {}", self.argument, self.message)
    }
}

impl InvalidArgumentError {
    /// Create a new invalid argument error.
    pub fn new(argument: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            argument: argument.into(),
            message: message.into(),
            cause: None,
        }
    }

    /// Add a cause to the error.
    pub fn with_cause(mut self, cause: Box<dyn std::error::Error + Send + Sync>) -> Self {
        self.cause = Some(cause);
        self
    }
}

#[cfg(test)]
#[path = "invalid_argument_error.test.rs"]
mod tests;
