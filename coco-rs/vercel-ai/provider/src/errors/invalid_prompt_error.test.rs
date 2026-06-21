use super::*;

#[test]
fn test_invalid_prompt_error_new() {
    let error = InvalidPromptError::new("Prompt cannot be empty");
    assert_eq!(error.message, "Prompt cannot be empty");
    assert!(error.prompt.is_none());
}

#[test]
fn test_invalid_prompt_error_with_prompt() {
    let prompt = serde_json::json!([{"role": "user", "content": "test"}]);
    let error = InvalidPromptError::new("Invalid message format").with_prompt(prompt.clone());
    assert_eq!(error.prompt, Some(prompt));
}

#[test]
fn test_invalid_prompt_error_display() {
    let error = InvalidPromptError::new("Missing required field");
    assert_eq!(format!("{error}"), "Invalid prompt: Missing required field");
}
