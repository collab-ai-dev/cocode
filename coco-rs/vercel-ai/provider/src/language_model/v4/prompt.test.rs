use super::*;

#[test]
fn test_message_system() {
    let msg = LanguageModelV4Message::system("You are helpful.");
    assert!(msg.is_system());
    assert!(!msg.is_developer());
    assert!(!msg.is_user());
    assert!(!msg.is_assistant());
    assert!(!msg.is_tool());
}

#[test]
fn test_message_developer_text() {
    let msg = LanguageModelV4Message::developer_text("Follow app policy.");
    assert!(msg.is_developer());
    assert!(!msg.is_system());
    assert!(!msg.is_user());
}

#[test]
fn test_message_user_text() {
    let msg = LanguageModelV4Message::user_text("Hello!");
    assert!(msg.is_user());
    assert!(!msg.is_system());
}

#[test]
fn test_message_assistant_text() {
    let msg = LanguageModelV4Message::assistant_text("Hi there!");
    assert!(msg.is_assistant());
    assert!(!msg.is_user());
}

#[test]
fn test_message_tool() {
    let msg = LanguageModelV4Message::tool(vec![]);
    assert!(msg.is_tool());
    assert!(!msg.is_assistant());
}

#[test]
fn test_prompt_builder_basic() {
    let prompt = PromptBuilder::new()
        .system("You are helpful.")
        .developer("Follow app policy.")
        .user("Hello!")
        .assistant("Hi!")
        .build();
    assert_eq!(prompt.len(), 4);
    assert!(prompt[0].is_system());
    assert!(prompt[1].is_developer());
    assert!(prompt[2].is_user());
    assert!(prompt[3].is_assistant());
}

#[test]
fn test_prompt_builder_empty() {
    let prompt = PromptBuilder::new().build();
    assert!(prompt.is_empty());
}

#[test]
fn test_message_with_options() {
    use crate::shared::ProviderOptions;
    let options = ProviderOptions::default();
    let msg = LanguageModelV4Message::system_with_options("System prompt", options);
    assert!(msg.is_system());
}

#[test]
fn test_prompt_serde() {
    let msg = LanguageModelV4Message::system("Test");
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("system"));
}
