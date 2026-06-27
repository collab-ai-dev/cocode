//! Load API key error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when an API key could not be loaded.
#[derive(Debug, Error)]
pub struct LoadAPIKeyError {
    /// The error message.
    pub message: String,
}

impl fmt::Display for LoadAPIKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failed to load API key: {}", self.message)
    }
}

impl LoadAPIKeyError {
    /// Create a new load API key error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Create an error for a missing environment variable.
    pub fn missing_env_var(env_var: &str) -> Self {
        Self {
            message: format!("API key not found. Please set the {env_var} environment variable."),
        }
    }
}

#[cfg(test)]
#[path = "load_api_key_error.test.rs"]
mod tests;
