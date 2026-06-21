//! Error type for when a provider is not found in the registry.

use thiserror::Error;
use vercel_ai_provider::NoSuchModelError;

/// Error thrown when a provider is not found in a registry.
#[derive(Error, Debug)]
#[error("{message}")]
pub struct NoSuchProviderError {
    /// The ID of the provider that was not found.
    pub provider_id: String,
    /// List of available provider IDs.
    pub available_providers: Vec<String>,
    /// The error message.
    pub message: String,
}

impl NoSuchProviderError {
    /// Create a new NoSuchProviderError.
    pub fn new(provider_id: impl Into<String>, available_providers: Vec<String>) -> Self {
        let provider_id = provider_id.into();
        let message = format!(
            "No such provider: {} (available providers: {})",
            provider_id,
            available_providers.join(", ")
        );

        Self {
            provider_id,
            available_providers,
            message,
        }
    }
}

impl From<NoSuchProviderError> for NoSuchModelError {
    fn from(err: NoSuchProviderError) -> Self {
        NoSuchModelError::new(err.message)
    }
}

#[cfg(test)]
#[path = "no_such_provider_error.test.rs"]
mod tests;
