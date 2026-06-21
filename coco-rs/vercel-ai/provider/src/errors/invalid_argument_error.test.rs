use super::*;

#[test]
fn test_invalid_argument_error_new() {
    let error = InvalidArgumentError::new("model_id", "cannot be empty");
    assert_eq!(error.argument, "model_id");
    assert_eq!(error.message, "cannot be empty");
    assert!(error.cause.is_none());
}

#[test]
fn test_invalid_argument_error_display() {
    let error = InvalidArgumentError::new("provider", "not found");
    assert_eq!(format!("{error}"), "Invalid argument 'provider': not found");
}

#[test]
fn test_invalid_argument_error_with_cause() {
    let cause = std::io::Error::other("inner error");
    let error = InvalidArgumentError::new("config", "invalid").with_cause(Box::new(cause));
    assert!(error.cause.is_some());
}
