//! No such model error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when a model is not found.
#[derive(Debug, Error)]
pub struct NoSuchModelError {
    /// The error message.
    pub message: String,
    /// The model ID that was not found.
    pub model_id: Option<String>,
    /// The type of model (e.g., "languageModel", "textEmbeddingModel").
    pub model_type: Option<String>,
}

impl fmt::Display for NoSuchModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "No such model: {}", self.message)
    }
}

impl NoSuchModelError {
    /// Create a new no such model error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            model_id: None,
            model_type: None,
        }
    }

    /// Create an error for a specific model ID.
    pub fn for_model(model_id: impl Into<String>) -> Self {
        let model_id = model_id.into();
        Self {
            message: format!("Model '{model_id}' not found"),
            model_id: Some(model_id),
            model_type: None,
        }
    }

    /// Create an error for a specific model ID and type.
    pub fn for_model_with_type(model_id: impl Into<String>, model_type: impl Into<String>) -> Self {
        let model_id = model_id.into();
        let model_type = model_type.into();
        Self {
            message: format!("No {model_type} with the id '{model_id}' is available"),
            model_id: Some(model_id),
            model_type: Some(model_type),
        }
    }
}

#[cfg(test)]
#[path = "no_such_model_error.test.rs"]
mod tests;
