//! Response handler traits and implementations.

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use vercel_ai_provider::AISdkError;

/// Trait for handling API responses.
#[async_trait]
pub trait ResponseHandler<T>: Send + Sync {
    /// Handle a successful response.
    async fn handle(
        &self,
        response: Response,
        url: &str,
        request_body_values: &Value,
    ) -> Result<T, AISdkError>;
}

/// Handler for JSON responses.
pub struct JsonResponseHandler<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> JsonResponseHandler<T> {
    /// Create a new JSON response handler.
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Default for JsonResponseHandler<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: DeserializeOwned + Send + Sync + 'static> ResponseHandler<T> for JsonResponseHandler<T> {
    async fn handle(
        &self,
        response: Response,
        _url: &str,
        _request_body_values: &Value,
    ) -> Result<T, AISdkError> {
        let body = response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read response: {e}")))?;

        serde_json::from_str(&body).map_err(|e| {
            AISdkError::new(format!("Failed to parse JSON: {e} (body: {body})"))
                .with_cause(Box::new(e))
        })
    }
}

/// Handler for text responses.
pub struct TextResponseHandler;

impl TextResponseHandler {
    /// Create a new text response handler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for TextResponseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponseHandler<String> for TextResponseHandler {
    async fn handle(
        &self,
        response: Response,
        _url: &str,
        _request_body_values: &Value,
    ) -> Result<String, AISdkError> {
        response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read response: {e}")))
    }
}

/// Handler for streaming responses.
pub struct StreamResponseHandler;

impl StreamResponseHandler {
    /// Create a new stream response handler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for StreamResponseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponseHandler<Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>>
    for StreamResponseHandler
{
    async fn handle(
        &self,
        response: Response,
        _url: &str,
        _request_body_values: &Value,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>, AISdkError> {
        Ok(Box::pin(response.bytes_stream()))
    }
}

/// Handler that returns the raw response.
pub struct RawResponseHandler;

impl RawResponseHandler {
    /// Create a new raw response handler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RawResponseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponseHandler<(reqwest::StatusCode, String)> for RawResponseHandler {
    async fn handle(
        &self,
        response: Response,
        _url: &str,
        _request_body_values: &Value,
    ) -> Result<(reqwest::StatusCode, String), AISdkError> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read response: {e}")))?;
        Ok((status, body))
    }
}

/// Blanket implementation allowing `Arc<dyn ResponseHandler<T>>` to be used
/// directly where `impl ResponseHandler<T>` is expected (e.g., API post functions).
#[async_trait]
impl<T: Send + 'static> ResponseHandler<T> for Arc<dyn ResponseHandler<T>> {
    async fn handle(
        &self,
        response: Response,
        url: &str,
        request_body_values: &Value,
    ) -> Result<T, AISdkError> {
        self.as_ref()
            .handle(response, url, request_body_values)
            .await
    }
}
