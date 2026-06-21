use super::*;

#[test]
fn test_error_message() {
    let error = NoSuchProviderError::new(
        "openai",
        vec!["anthropic".to_string(), "google".to_string()],
    );

    assert_eq!(error.provider_id, "openai");
    assert_eq!(error.available_providers, vec!["anthropic", "google"]);
    assert!(error.message.contains("No such provider: openai"));
}

#[test]
fn test_to_string() {
    let error = NoSuchProviderError::new("openai", vec!["anthropic".to_string()]);

    let msg = error.to_string();
    assert!(msg.contains("No such provider: openai"));
    assert!(msg.contains("anthropic"));
}
