use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider_utils::ResponseHandler;

/// Anthropic error response shape: `{ type: "error", error: { type, message } }`.
#[derive(Debug, Deserialize)]
pub struct AnthropicErrorData {
    pub error: AnthropicErrorDetail,
}

/// Inner error detail from Anthropic API.
#[derive(Debug, Deserialize)]
pub struct AnthropicErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

/// Error response handler that parses Anthropic error JSON and wraps it in `AISdkError`.
pub struct AnthropicFailedResponseHandler;

#[async_trait]
impl ResponseHandler<AISdkError> for AnthropicFailedResponseHandler {
    async fn handle(
        &self,
        response: Response,
        url: &str,
        _request_body_values: &Value,
    ) -> Result<AISdkError, AISdkError> {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| String::from("<failed to read body>"));

        let message = match serde_json::from_str::<AnthropicErrorData>(&body) {
            Ok(data) => data.error.message,
            Err(_) => {
                // Fall back to generic error message extraction
                vercel_ai_provider_utils::get_error_message(
                    &serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone())),
                )
            }
        };

        // Attach a typed `APICallError` cause carrying the HTTP status (as
        // OpenAI/Google do). Without it, `coco-inference::wrap_provider_error`
        // can't recover the status and every Anthropic error — the PRIMARY
        // provider — collapses to a non-retryable `ProviderError { status: 0 }`,
        // killing the backoff-retry and reactive-401 paths for 429/5xx/401.
        let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;
        let api_error = APICallError::new(&message, url)
            .with_status(status.as_u16())
            .with_response_body(&body)
            .with_retryable(is_retryable);

        Ok(
            AISdkError::new(format!("Anthropic API error ({status}): {message}"))
                .with_cause(Box::new(api_error)),
        )
    }
}

#[cfg(test)]
#[path = "anthropic_error.test.rs"]
mod tests;
