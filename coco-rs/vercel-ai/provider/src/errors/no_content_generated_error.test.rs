use super::*;

#[test]
fn test_no_content_generated_error_new() {
    let error = NoContentGeneratedError::new("The model did not generate any content");
    assert_eq!(error.message, "The model did not generate any content");
}

#[test]
fn test_no_content_generated_error_default() {
    let error = NoContentGeneratedError::default();
    assert_eq!(error.message, "No content generated.");
}

#[test]
fn test_no_content_generated_error_display() {
    let error = NoContentGeneratedError::default();
    assert_eq!(format!("{error}"), "No content generated.");
}
