use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider_utils::ResponseHandler;

/// The exact `code` value xAI returns to mark a soft error (delivered with
/// HTTP 200, not an HTTP error status) as a transient outage the client should
/// retry. Mirrors the TS `isRetryable: code === 'The service is currently
/// unavailable'` check on both the `do_generate` and `do_stream` soft-error
/// paths.
pub const SERVICE_UNAVAILABLE_CODE: &str = "The service is currently unavailable";

/// xAI error response shapes.
///
/// The Chat Completions API returns `{ "error": { "message", "type", ... } }`,
/// while the Responses API returns `{ "code": ..., "error": ... }`. Mirrors the
/// `xaiErrorDataSchema` union in `xai-error.ts`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum XaiErrorData {
    /// Chat Completions error: `{ error: { message, type, param, code } }`.
    ChatCompletions { error: XaiChatCompletionsError },
    /// Responses API error: `{ code, error }`.
    Responses { code: String, error: String },
}

/// Inner error detail from the Chat Completions API.
#[derive(Debug, Deserialize)]
pub struct XaiChatCompletionsError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

impl XaiErrorData {
    /// Render a human-readable message, matching `errorToMessage` in the TS
    /// `xaiFailedResponseHandler` (the HTTP-error path prefixes the `code`).
    pub fn to_message(&self) -> String {
        match self {
            XaiErrorData::ChatCompletions { error } => error.message.clone(),
            XaiErrorData::Responses { code, error } => format!("{code}: {error}"),
        }
    }

    /// The raw error text with no `code` prefix. The soft-error (HTTP 200) and
    /// stream JSON-error paths surface the message verbatim — unlike the
    /// HTTP-error handler (`to_message`), matching the TS.
    pub fn error_text(&self) -> &str {
        match self {
            XaiErrorData::ChatCompletions { error } => &error.message,
            XaiErrorData::Responses { error, .. } => error,
        }
    }

    /// The `code` field, present only on the Responses-shaped `{code, error}`
    /// body. Drives the exact-match retryability check on soft errors.
    pub fn code(&self) -> Option<&str> {
        match self {
            XaiErrorData::Responses { code, .. } => Some(code),
            XaiErrorData::ChatCompletions { .. } => None,
        }
    }
}

/// Error response handler that parses xAI error JSON and wraps it in `AISdkError`.
pub struct XaiFailedResponseHandler;

impl XaiFailedResponseHandler {
    /// Create a new handler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for XaiFailedResponseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponseHandler<AISdkError> for XaiFailedResponseHandler {
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

        let message = match serde_json::from_str::<XaiErrorData>(&body) {
            Ok(data) => data.to_message(),
            Err(_) => vercel_ai_provider_utils::get_error_message(
                &serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone())),
            ),
        };

        // Carry the HTTP status on an `APICallError` cause. The inference retry
        // classifier downcasts `AISdkError.cause` to recover the status and
        // decide retryability (429/503/529 → retry); without it a transient
        // 5xx would degrade to a non-retryable error on the blocking path.
        let api_err = APICallError::new(message.clone(), url)
            .with_status(status.as_u16())
            .with_response_body(body);

        Ok(
            AISdkError::new(format!("xAI API error ({status}): {message}"))
                .with_cause(Box::new(api_err)),
        )
    }
}

#[cfg(test)]
#[path = "xai_error.test.rs"]
mod tests;
