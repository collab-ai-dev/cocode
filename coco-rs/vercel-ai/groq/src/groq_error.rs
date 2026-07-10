use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider_utils::ResponseHandler;

/// Groq error response shape: `{ "error": { "message": ..., "type": ... } }`.
#[derive(Debug, Deserialize)]
pub struct GroqErrorData {
    pub error: GroqErrorDetail,
}

/// Inner error detail from the Groq API.
#[derive(Debug, Deserialize)]
pub struct GroqErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

/// Error response handler that parses Groq error JSON and wraps it in `AISdkError`.
pub struct GroqFailedResponseHandler;

impl GroqFailedResponseHandler {
    /// Create a new handler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for GroqFailedResponseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponseHandler<AISdkError> for GroqFailedResponseHandler {
    async fn handle(
        &self,
        response: Response,
        _url: &str,
        _request_body_values: &Value,
    ) -> Result<AISdkError, AISdkError> {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| String::from("<failed to read body>"));

        let message = match serde_json::from_str::<GroqErrorData>(&body) {
            Ok(data) => data.error.message,
            Err(_) => vercel_ai_provider_utils::get_error_message(
                &serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone())),
            ),
        };

        Ok(AISdkError::new(format!(
            "Groq API error ({status}): {message}"
        )))
    }
}

#[cfg(test)]
#[path = "groq_error.test.rs"]
mod tests;
