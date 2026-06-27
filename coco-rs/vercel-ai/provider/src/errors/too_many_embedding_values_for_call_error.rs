//! Too many embedding values for call error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when too many values are provided for a single embedding call.
#[derive(Debug, Error)]
pub struct TooManyEmbeddingValuesForCallError {
    /// The provider name.
    pub provider: String,
    /// The model ID.
    pub model_id: String,
    /// Maximum embeddings per call.
    pub max_embeddings_per_call: usize,
    /// The number of values that were provided.
    pub values_count: usize,
    /// The error message.
    pub message: String,
}

impl fmt::Display for TooManyEmbeddingValuesForCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl TooManyEmbeddingValuesForCallError {
    /// Create a new too many embedding values error.
    pub fn new(
        provider: impl Into<String>,
        model_id: impl Into<String>,
        max_embeddings_per_call: usize,
        values: usize,
    ) -> Self {
        let provider = provider.into();
        let model_id = model_id.into();
        let message = format!(
            "Too many values for a single embedding call. The {provider} model \"{model_id}\" can only embed up to {max_embeddings_per_call} values per call, but {values} values were provided."
        );
        Self {
            provider,
            model_id,
            max_embeddings_per_call,
            values_count: values,
            message,
        }
    }
}

#[cfg(test)]
#[path = "too_many_embedding_values_for_call_error.test.rs"]
mod tests;
