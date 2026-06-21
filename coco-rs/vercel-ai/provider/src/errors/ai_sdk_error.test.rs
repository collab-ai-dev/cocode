use super::*;

#[test]
fn test_ai_sdk_error_new() {
    let error = AISdkError::new("Something went wrong");
    assert_eq!(error.message, "Something went wrong");
    assert!(error.cause.is_none());
}

#[test]
fn test_ai_sdk_error_display() {
    let error = AISdkError::new("Test message");
    assert_eq!(format!("{error}"), "Test message");
}

#[test]
fn test_ai_sdk_error_with_cause() {
    let cause = std::io::Error::other("inner error");
    let error = AISdkError::new("outer error").with_cause(Box::new(cause));
    assert!(error.cause.is_some());
}

#[test]
fn test_ai_sdk_error_debug() {
    let error = AISdkError::new("test");
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("AISdkError"));
}
