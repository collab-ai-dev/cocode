//! Google API error types and response handler.

use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider_utils::ResponseHandler;

/// Google API error response structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleErrorData {
    pub error: GoogleErrorDetail,
}

/// Detail of a Google API error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleErrorDetail {
    pub code: Option<i32>,
    pub message: Option<String>,
    pub status: Option<String>,
}

/// Response handler for Google API error responses.
pub struct GoogleFailedResponseHandler;

#[async_trait]
impl ResponseHandler<AISdkError> for GoogleFailedResponseHandler {
    async fn handle(
        &self,
        response: Response,
        url: &str,
        _request_body_values: &Value,
    ) -> Result<AISdkError, AISdkError> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;

        let message = if let Ok(error_data) = serde_json::from_str::<GoogleErrorData>(&body) {
            error_data
                .error
                .message
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
