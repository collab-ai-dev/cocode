//! Invalid response data error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when server returns a response with invalid data content.
///
/// This should be thrown by providers when they cannot parse the response from the API.
#[derive(Debug, Error)]
pub struct InvalidResponseDataError {
    /// The invalid data that was received.
    pub data: serde_json::Value,
    /// The error message.
    pub message: String,
}

impl fmt::Display for InvalidResponseDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl InvalidResponseDataError {
    /// Create a new invalid response data error.
    pub fn new(data: serde_json::Value) -> Self {
        let message = format!(
            "Invalid response data: {}",
            serde_json::to_string(&data).unwrap_or_default()
        );
        Self { data, message }
    }

    /// Create with a custom message.
    pub fn with_message(data: serde_json::Value, message: impl Into<String>) -> Self {
        Self {
            data,
            message: message.into(),
        }
    }
}

#[cfg(test)]
#[path = "invalid_response_data_error.test.rs"]
mod tests;
