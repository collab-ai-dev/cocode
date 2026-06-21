//! No such provider reference error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when a provider reference cannot be resolved by a provider.
///
/// This occurs when a `SharedV4ProviderReference` map does not contain an
/// entry for the current provider, i.e. the file has not been uploaded to
/// this provider.
#[derive(Debug, Error)]
pub struct NoSuchProviderReferenceError {
    /// The error message.
    pub message: String,
    /// The provider ID that was not found in the reference map.
    pub provider_id: Option<String>,
}

impl fmt::Display for NoSuchProviderReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoSuchProviderReferenceError: {}", self.message)
    }
}

impl NoSuchProviderReferenceError {
    /// Create a new error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            provider_id: None,
        }
    }

    /// Create an error for a specific provider ID.
    pub fn for_provider(provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        Self {
            message: format!("Provider reference not found for provider '{provider_id}'"),
            provider_id: Some(provider_id),
        }
    }
}
