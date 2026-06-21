use super::*;

#[test]
fn test_prompt_from_str() {
    let prompt: Prompt = "Hello, world!".into();
    assert!(prompt.system.is_none());
}

#[test]
fn test_prompt_with_system() {
    let prompt = Prompt::user("Hello").with_system("Be helpful.");
    assert!(prompt.system.is_some());
}

#[test]
fn test_language_model_from_string() {
    let model: LanguageModel = "gpt-4".into();
    assert!(model.is_string());
}

#[test]
fn test_provider_registry_with_mock_provider() {
    use std::sync::Arc;

    let mock_model = test_utils::MockLanguageModel::with_text("Hello from mock");
    let provider =
        test_utils::MockProvider::new().with_language_model("test-model", Arc::new(mock_model));

    let resolved = provider.language_model("test-model");
    assert!(resolved.is_ok());

    let not_found = provider.language_model("nonexistent");
    assert!(not_found.is_err());
}

#[test]
fn test_language_model_from_v4() {
    use std::sync::Arc;

    let mock_model = test_utils::MockLanguageModel::with_text("Test response");
    let model = LanguageModel::from_v4(Arc::new(mock_model));

    assert!(model.is_v4());
    assert!(!model.is_string());
    assert!(model.as_string().is_none());
}

#[test]
fn test_prompt_with_messages() {
    use vercel_ai_provider::LanguageModelV4Message;

    let messages = vec![
        LanguageModelV4Message::user_text("What is Rust?"),
        LanguageModelV4Message::assistant_text("Rust is a systems programming language."),
        LanguageModelV4Message::user_text("Tell me more."),
    ];

    let prompt = Prompt::messages(messages);
    assert!(prompt.system.is_none());

    let model_prompt = prompt.to_model_prompt();
    assert_eq!(model_prompt.len(), 3);
}
