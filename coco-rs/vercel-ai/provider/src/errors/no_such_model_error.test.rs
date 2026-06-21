use super::*;

#[test]
fn test_no_such_model_error_new() {
    let error = NoSuchModelError::new("Model not available");
    assert_eq!(error.message, "Model not available");
    assert!(error.model_id.is_none());
}

#[test]
fn test_no_such_model_error_for_model() {
    let error = NoSuchModelError::for_model("gpt-5");
    assert_eq!(error.model_id, Some("gpt-5".to_string()));
    assert!(error.message.contains("gpt-5"));
}

#[test]
fn test_no_such_model_error_display() {
    let error = NoSuchModelError::for_model("claude-4");
    assert_eq!(
        format!("{error}"),
        "No such model: Model 'claude-4' not found"
    );
}
