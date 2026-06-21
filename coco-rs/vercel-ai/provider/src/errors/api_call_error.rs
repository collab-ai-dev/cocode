//! API call error type.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use thiserror::Error;

/// Error thrown when an API call fails.
#[derive(Debug, Error)]
pub struct APICallError {
    /// The error message.
    pub message: String,
    /// The URL that was called.
    pub url: String,
    /// The HTTP status code (if available).
    pub status_code: Option<u16>,
    /// The response body (if available).
    pub response_body: Option<String>,
    /// The underlying cause (if any).
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// Whether this error is retryable.
    pub is_retryable: bool,
    /// Suggested delay before retry (if retryable).
    pub retry_after: Option<Duration>,
    /// The data that was sent to the API.
    pub data: Option<serde_json::Value>,
    /// The request body values (for debugging).
    pub request_body_values: Option<serde_json::Value>,
    /// Response headers from the failed request.
    pub response_headers: Option<HashMap<String, String>>,
}

impl fmt::Display for APICallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "API call error: {}", self.message)?;
        if let Some(status) = self.status_code {
            write!(f, " (status {status})")?;
        }
        Ok(())
    }
}

impl APICallError {
    /// Create a new API call error.
    ///
    /// Default `is_retryable` is determined by the status code:
    /// retryable for 408, 409, 429, and >= 500.
    pub fn new(message: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            url: url.into(),
            status_code: None,
            response_body: None,
            cause: None,
            is_retryable: false,
            retry_after: None,
            data: None,
            request_body_values: None,
            response_headers: None,
        }
    }

    /// Create a retryable API call error.
    pub fn retryable(message: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            url: url.into(),
            status_code: None,
            response_body: None,
            cause: None,
            is_retryable: true,
            retry_after: None,
            data: None,
            request_body_values: None,
            response_headers: None,
        }
    }

    /// Set the HTTP status code.
    ///
    /// Also updates `is_retryable` based on status code if not explicitly set:
    /// 408 (Timeout), 409 (Conflict), 429 (Too Many Requests), >= 500 are retryable.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status_code = Some(status);
        // Apply default retryable logic based on status code
        self.is_retryable =
            self.is_retryable || status == 408 || status == 409 || status == 429 || status >= 500;
        self
    }

    /// Set the response body.
    pub fn with_response_body(mut self, body: impl Into<String>) -> Self {
        self.response_body = Some(body.into());
        self
    }

    /// Set whether this error is retryable.
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.is_retryable = retryable;
        self
    }

    /// Set the retry delay.
    pub fn with_retry_after(mut self, duration: Duration) -> Self {
        self.retry_after = Some(duration);
        self
    }

    /// Set the data that was sent.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Set the request body values.
    pub fn with_request_body_values(mut self, values: serde_json::Value) -> Self {
        self.request_body_values = Some(values);
        self
    }

    /// Set the response headers.
    pub fn with_response_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.response_headers = Some(headers);
        self
    }
}

#[cfg(test)]
#[path = "api_call_error.test.rs"]
mod tests;
