//! ByteDance API error types and response handler.

use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider_utils::ResponseHandler;

/// ByteDance API error response structure.
///
/// ByteDance errors may have either a nested `error.message` or a top-level `message`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteDanceErrorData {
    /// Nested error object (optional).
    pub error: Option<ByteDanceErrorDetail>,
    /// Top-level error message (optional).
    pub message: Option<String>,
}

/// Detail of a ByteDance API error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteDanceErrorDetail {
    pub code: Option<String>,
    pub message: Option<String>,
}

/// Response handler for ByteDance API error responses.
pub struct ByteDanceFailedResponseHandler;

#[async_trait]
impl ResponseHandler<AISdkError> for ByteDanceFailedResponseHandler {
    async fn handle(
        &self,
        response: Response,
        url: &str,
        _request_body_values: &Value,
    ) -> Result<AISdkError, AISdkError> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;

        let message = if let Ok(error_data) = serde_json::from_str::<ByteDanceErrorData>(&body) {
            // Try nested error.message first, then top-level message
            error_data
                .error
                .and_then(|e| e.message)
                .or(error_data.message)
                .unwrap_or_else(|| format!("HTTP {status}"))
        } else {
            format!("HTTP {status}: {body}")
        };

        let api_error = APICallError::new(message, url)
            .with_status(status.as_u16())
            .with_response_body(body)
            .with_retryable(is_retryable);

        Ok(AISdkError::new(api_error.message.clone()).with_cause(Box::new(api_error)))
    }
}
