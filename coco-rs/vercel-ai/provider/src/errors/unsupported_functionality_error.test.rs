use super::*;

#[test]
fn test_unsupported_functionality_error_new() {
    let error = UnsupportedFunctionalityError::new("streaming");
    assert_eq!(error.functionality, "streaming");
    assert!(error.message.contains("streaming"));
    assert!(error.message.contains("not supported"));
}

#[test]
fn test_unsupported_functionality_error_with_message() {
    let error = UnsupportedFunctionalityError::with_message(
        "vision",
        "This model does not support image inputs",
    );
    assert_eq!(error.functionality, "vision");
    assert_eq!(error.message, "This model does not support image inputs");
}

#[test]
fn test_unsupported_functionality_error_display() {
    let error = UnsupportedFunctionalityError::new("function calling");
    let display = format!("{error}");
    assert!(display.contains("Unsupported functionality"));
}
