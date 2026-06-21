//! Empty response body error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when an API response has an empty body.
#[derive(Debug, Error)]
pub struct EmptyResponseBodyError {
    /// The error message.
    pub message: String,
}

impl fmt::Display for EmptyResponseBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Empty response body: {}", self.message)
    }
}

impl EmptyResponseBodyError {
    /// Create a new empty response body error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Default for EmptyResponseBodyError {
    fn default() -> Self {
        Self {
            message: "Empty response body".to_string(),
        }
    }
}

#[cfg(test)]
#[path = "empty_response_body_error.test.rs"]
mod tests;
