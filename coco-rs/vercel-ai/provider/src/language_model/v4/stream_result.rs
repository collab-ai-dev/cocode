//! Language model stream result (V4).

use futures::Stream;
use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;

use super::generate_result::LanguageModelV4Request;
use super::stream::LanguageModelV4StreamPart;
use crate::errors::AISdkError;

/// Response metadata available at stream initialization time.
#[derive(Debug, Clone, Default)]
pub struct LanguageModelV4StreamResponse {
    /// Response headers from the provider.
    pub headers: Option<HashMap<String, String>>,
}

impl LanguageModelV4StreamResponse {
    /// Create a new stream response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the response headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }
}

/// The result of a stream call.
pub struct LanguageModelV4StreamResult {
    /// The stream of parts.
    pub stream: Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>>,
    /// Request information (for telemetry).
    pub request: Option<LanguageModelV4Request>,
    /// Response information available at stream initialization.
    pub response: Option<LanguageModelV4StreamResponse>,
}

impl fmt::Debug for LanguageModelV4StreamResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LanguageModelV4StreamResult")
            .field("stream", &"<stream>")
            .field("request", &self.request)
            .field("response", &self.response)
            .finish()
    }
}

impl LanguageModelV4StreamResult {
    /// Create a new stream result.
    pub fn new(
        stream: Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>>,
    ) -> Self {
        Self {
            stream,
            request: None,
            response: None,
        }
    }

    /// Set request information.
    pub fn with_request(mut self, request: LanguageModelV4Request) -> Self {
        self.request = Some(request);
        self
    }

    /// Set response information.
    pub fn with_response(mut self, response: LanguageModelV4StreamResponse) -> Self {
        self.response = Some(response);
        self
    }
}

#[cfg(test)]
#[path = "stream_result.test.rs"]
mod tests;
