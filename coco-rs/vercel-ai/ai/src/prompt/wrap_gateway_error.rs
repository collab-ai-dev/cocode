//! Gateway error wrapping utilities.
//!
//! This module provides utilities for wrapping provider errors
//! into user-friendly error messages.

use vercel_ai_provider::AISdkError;

use crate::error::AIError;

/// Wrap a gateway/provider error with additional context.
///
/// This function converts provider-specific errors into user-friendly
/// AIError types with additional context about where the error occurred.
///
/// # Arguments
///
/// * `error` - The original error from the provider.
/// * `provider_name` - The name of the provider (e.g., "openai", "anthropic").
/// * `model_id` - The model ID being used.
///
/// # Returns
///
/// An `AIError` with wrapped error context.
pub fn wrap_gateway_error(error: AISdkError, provider_name: &str, model_id: &str) -> AIError {
    let message = format!("Provider '{provider_name}' error for model '{model_id}': {error}");

    AIError::ProviderError(AISdkError::new(message))
}

/// Wrap a gateway error with a custom message.
///
/// # Arguments
///
/// * `error` - The original error.
/// * `provider_name` - The provider name.
/// * `model_id` - The model ID.
/// * `context` - Additional context message.
///
/// # Returns
///
/// An `AIError` with custom context.
pub fn wrap_gateway_error_with_context(
    error: AISdkError,
    provider_name: &str,
    model_id: &str,
    context: &str,
) -> AIError {
    let message = format!(
        "Provider '{provider_name}' error for model '{model_id}' during {context}: {error}"
    );

    AIError::ProviderError(AISdkError::new(message))
}

/// Check if an error is retryable.
///
/// Some provider errors (like rate limits or temporary failures) can be retried.
///
/// # Arguments
///
/// * `error` - The error to check.
///
/// # Returns
///
/// `true` if the error might succeed on retry, `false` otherwise.
pub fn is_gateway_error_retryable(error: &AIError) -> bool {
    match error {
        AIError::ProviderError(e) => {
            let msg = &e.message;
            // Rate limit and server errors are typically retryable
            msg.contains("rate_limit_exceeded")
                || msg.contains("server_error")
                || msg.contains("overloaded")
                || msg.contains("timeout")
        }
        _ => false,
    }
}

/// Get a user-friendly error message.
///
/// # Arguments
///
/// * `error` - The error to format.
///
/// # Returns
///
/// A user-friendly error message string.
pub fn get_user_friendly_message(error: &AIError) -> String {
    match error {
        AIError::ProviderError(e) => {
            let msg = &e.message;
            if msg.contains("invalid_api_key") {
                "Invalid API key. Please check your API key configuration.".to_string()
            } else if msg.contains("rate_limit_exceeded") {
                "Rate limit exceeded. Please wait a moment and try again.".to_string()
            } else if msg.contains("insufficient_quota") {
                "Insufficient quota. Please check your account limits.".to_string()
            } else if msg.contains("model_not_found") {
                "Model not found. Please check the model name.".to_string()
            } else if msg.contains("context_length_exceeded") {
                "Context length exceeded. Please reduce the size of your input.".to_string()
            } else if msg.contains("invalid_request") {
                format!("Invalid request: {e}")
            } else {
                format!("Provider error: {e}")
            }
        }
        AIError::NoOutputGenerated => "No output was generated. Please try again.".to_string(),
        AIError::SchemaValidation(msg) => format!("Schema validation failed: {msg}"),
        _ => error.to_string(),
    }
}

#[cfg(test)]
#[path = "wrap_gateway_error.test.rs"]
mod tests;
