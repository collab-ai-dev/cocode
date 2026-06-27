use super::super::call_settings::CallSettings;
use super::*;

#[test]
fn test_prompt_from_text() {
    let prompt = Prompt::user("Hello, world!");
    assert!(prompt.system.is_none());
    match prompt.content {
        PromptContent::Text(text) => assert_eq!(text, "Hello, world!"),
        _ => panic!("Expected text content"),
    }
}

#[test]
fn test_prompt_with_system() {
    let prompt = Prompt::user("Hello").with_system("You are a helpful assistant.");
    assert!(prompt.system.is_some());
    match prompt.system {
        Some(SystemPrompt::Text(text)) => {
            assert_eq!(text, "You are a helpful assistant.");
        }
        _ => panic!("Expected text system prompt"),
    }
}

#[test]
fn test_prompt_to_model_prompt() {
    let prompt = Prompt::user("Hello").with_system("You are helpful.");
    let model_prompt = prompt.to_model_prompt();
    assert_eq!(model_prompt.len(), 2);
    assert!(model_prompt[0].is_system());
    assert!(model_prompt[1].is_user());
}

#[test]
fn test_call_settings_builder() {
    let settings = CallSettings::new()
        .with_max_tokens(100)
        .with_temperature(0.7)
        .with_top_p(0.9);
    assert_eq!(settings.max_tokens, Some(100));
    assert_eq!(settings.temperature, Some(0.7));
    assert_eq!(settings.top_p, Some(0.9));
}
