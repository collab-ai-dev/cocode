use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider_utils::ResponseHandler;

/// OpenAI-compatible error response shape.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleErrorData {
    pub error: OpenAICompatibleErrorDetail,
}

/// Inner error detail from an OpenAI-compatible API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub param: Option<Value>,
    pub code: Option<Value>,
}

/// Error response handler that parses OpenAI-compatible error JSON and wraps it in `AISdkError`.
///
/// The provider name is included in the error message for better diagnostics.
pub struct OpenAICompatibleFailedResponseHandler {
    provider_name: String,
}

impl OpenAICompatibleFailedResponseHandler {
    /// Create a new handler with the given provider name (e.g., "xAI", "Groq").
    pub fn new(provider_name: impl Into<String>) -> Self {
        Self {
            provider_name: provider_name.into(),
        }
    }
}

#[async_trait]
impl ResponseHandler<AISdkError> for OpenAICompatibleFailedResponseHandler {
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

        let message = match serde_json::from_str::<OpenAICompatibleErrorData>(&body) {
            Ok(data) => data.error.message,
            Err(_) => {
                // Fall back to generic error message extraction
                vercel_ai_provider_utils::get_error_message(
                    &serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone())),
                )
            }
        };

        Ok(AISdkError::new(format!(
            "{} API error ({status}): {message}",
            self.provider_name
        )))
    }
}

#[cfg(test)]
#[path = "openai_compatible_error.test.rs"]
mod tests;
