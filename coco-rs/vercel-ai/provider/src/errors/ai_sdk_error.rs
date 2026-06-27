//! Base AI SDK error type.

use std::fmt;
use thiserror::Error;

/// The base error type for all AI SDK errors.
#[derive(Debug, Error)]
pub struct AISdkError {
    /// The error message.
    pub message: String,
    /// The underlying cause (if any).
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for AISdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl AISdkError {
    /// Create a new AISdkError with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
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

impl From<super::provider_error::ProviderError> for AISdkError {
    fn from(err: super::provider_error::ProviderError) -> Self {
        let message = err.to_string();
        Self {
            message,
            cause: Some(Box::new(err)),
        }
    }
}

#[cfg(test)]
#[path = "ai_sdk_error.test.rs"]
mod tests;
