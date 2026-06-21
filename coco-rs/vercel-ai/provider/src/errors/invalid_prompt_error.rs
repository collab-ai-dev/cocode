//! Invalid prompt error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when the prompt is invalid.
#[derive(Debug, Error)]
pub struct InvalidPromptError {
    /// The error message.
    pub message: String,
    /// The prompt that caused the error.
    pub prompt: Option<serde_json::Value>,
}

impl fmt::Display for InvalidPromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid prompt: {}", self.message)
    }
}

impl InvalidPromptError {
    /// Create a new invalid prompt error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            prompt: None,
        }
    }

    /// Set the prompt that caused the error.
    pub fn with_prompt(mut self, prompt: serde_json::Value) -> Self {
        self.prompt = Some(prompt);
        self
    }
}

#[cfg(test)]
#[path = "invalid_prompt_error.test.rs"]
mod tests;
