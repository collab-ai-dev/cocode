use super::*;

#[test]
fn test_empty_response_body_error_new() {
    let error = EmptyResponseBodyError::new("Custom message");
    assert_eq!(error.message, "Custom message");
    assert_eq!(format!("{error}"), "Empty response body: Custom message");
}

#[test]
fn test_empty_response_body_error_default() {
    let error = EmptyResponseBodyError::default();
    assert_eq!(error.message, "Empty response body");
}

#[test]
fn test_empty_response_body_error_debug() {
    let error = EmptyResponseBodyError::new("test");
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("EmptyResponseBodyError"));
}
