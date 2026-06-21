use vercel_ai_provider::AISdkError;

use super::*;
use crate::error::AIError;

#[test]
fn test_wrap_gateway_error_includes_provider_info() {
    let error = AISdkError::new("something went wrong");
    let wrapped = wrap_gateway_error(error, "openai", "gpt-4");
    let msg = wrapped.to_string();
    assert!(msg.contains("openai"));
    assert!(msg.contains("gpt-4"));
}

#[test]
fn test_wrap_gateway_error_with_context() {
    let error = AISdkError::new("timeout");
    let wrapped = wrap_gateway_error_with_context(error, "anthropic", "claude-3", "streaming");
    let msg = wrapped.to_string();
    assert!(msg.contains("anthropic"));
    assert!(msg.contains("claude-3"));
    assert!(msg.contains("streaming"));
}

#[test]
fn test_is_retryable_rate_limit() {
    let error = AIError::ProviderError(AISdkError::new("rate_limit_exceeded"));
    assert!(is_gateway_error_retryable(&error));
}

#[test]
fn test_is_retryable_server_error() {
    let error = AIError::ProviderError(AISdkError::new("server_error: internal"));
    assert!(is_gateway_error_retryable(&error));
}

#[test]
fn test_is_retryable_overloaded() {
    let error = AIError::ProviderError(AISdkError::new("model overloaded"));
    assert!(is_gateway_error_retryable(&error));
}

#[test]
fn test_is_not_retryable_invalid_key() {
    let error = AIError::ProviderError(AISdkError::new("invalid_api_key"));
    assert!(!is_gateway_error_retryable(&error));
}

#[test]
fn test_is_not_retryable_non_provider() {
    let error = AIError::NoOutputGenerated;
    assert!(!is_gateway_error_retryable(&error));
}

#[test]
fn test_friendly_message_invalid_key() {
    let error = AIError::ProviderError(AISdkError::new("invalid_api_key"));
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("API key"));
}

#[test]
fn test_friendly_message_rate_limit() {
    let error = AIError::ProviderError(AISdkError::new("rate_limit_exceeded"));
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("Rate limit"));
}

#[test]
fn test_friendly_message_quota() {
    let error = AIError::ProviderError(AISdkError::new("insufficient_quota"));
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("quota"));
}

#[test]
fn test_friendly_message_model_not_found() {
    let error = AIError::ProviderError(AISdkError::new("model_not_found"));
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("Model not found"));
}

#[test]
fn test_friendly_message_no_output() {
    let error = AIError::NoOutputGenerated;
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("No output"));
}

#[test]
fn test_friendly_message_schema_validation() {
    let error = AIError::SchemaValidation("bad schema".to_string());
    let msg = get_user_friendly_message(&error);
    assert!(msg.contains("Schema validation"));
}
