use super::*;

#[test]
fn test_load_api_key_error_new() {
    let error = LoadAPIKeyError::new("API key file not found");
    assert_eq!(error.message, "API key file not found");
}

#[test]
fn test_load_api_key_error_missing_env_var() {
    let error = LoadAPIKeyError::missing_env_var("OPENAI_API_KEY");
    assert!(error.message.contains("OPENAI_API_KEY"));
    assert!(error.message.contains("API key not found"));
}

#[test]
fn test_load_api_key_error_display() {
    let error = LoadAPIKeyError::missing_env_var("ANTHROPIC_API_KEY");
    assert!(format!("{error}").contains("Failed to load API key"));
}
