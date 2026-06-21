use super::*;
use std::time::Duration;

#[test]
fn test_api_call_error_new() {
    let error = APICallError::new("Request failed", "https://api.example.com/v1/chat");
    assert_eq!(error.message, "Request failed");
    assert_eq!(error.url, "https://api.example.com/v1/chat");
    assert!(error.status_code.is_none());
    assert!(error.response_body.is_none());
    assert!(!error.is_retryable);
}

#[test]
fn test_api_call_error_retryable() {
    let error = APICallError::retryable("Timeout", "https://api.example.com");
    assert!(error.is_retryable);
}

#[test]
fn test_api_call_error_with_status() {
    let error = APICallError::new("Error", "https://api.example.com").with_status(500);
    assert_eq!(error.status_code, Some(500));
}

#[test]
fn test_api_call_error_with_response_body() {
    let error = APICallError::new("Error", "https://api.example.com")
        .with_response_body("{\"error\": \"Internal Server Error\"}");
    assert_eq!(
        error.response_body,
        Some("{\"error\": \"Internal Server Error\"}".to_string())
    );
}

#[test]
fn test_api_call_error_with_retry_after() {
    let error = APICallError::retryable("Rate limited", "https://api.example.com")
        .with_retry_after(Duration::from_secs(30));
    assert_eq!(error.retry_after, Some(Duration::from_secs(30)));
}

#[test]
fn test_api_call_error_display() {
    let error = APICallError::new("Request failed", "https://api.example.com").with_status(404);
    assert_eq!(
        format!("{error}"),
        "API call error: Request failed (status 404)"
    );
}
