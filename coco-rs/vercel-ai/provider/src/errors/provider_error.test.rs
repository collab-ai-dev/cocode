use super::*;

#[test]
fn test_from_api_call_error() {
    let api_err = APICallError::new("connection refused", "https://api.example.com");
    let err: ProviderError = api_err.into();
    assert!(matches!(err, ProviderError::ApiCall(_)));
    assert!(!err.is_retryable());
}

#[test]
fn test_from_no_such_model_error() {
    let model_err = NoSuchModelError::for_model("gpt-5");
    let err: ProviderError = model_err.into();
    assert!(matches!(err, ProviderError::NoSuchModel(_)));
}

#[test]
fn test_retryable_api_call() {
    let api_err =
        APICallError::retryable("rate limited", "https://api.example.com").with_status(429);
    let err: ProviderError = api_err.into();
    assert!(err.is_retryable());
}
