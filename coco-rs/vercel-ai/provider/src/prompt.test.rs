use super::*;

#[test]
fn test_prompt_builder() {
    let prompt = PromptBuilder::new()
        .system("You are a helpful assistant.")
        .user("Hello!")
        .build();

    assert_eq!(prompt.len(), 2);
    assert!(prompt[0].is_system());
    assert!(prompt[1].is_user());
}

#[test]
fn test_message_creation() {
    let system = LanguageModelV4Message::system("System prompt");
    assert!(system.is_system());

    let user = LanguageModelV4Message::user_text("User message");
    assert!(user.is_user());

    let assistant = LanguageModelV4Message::assistant_text("Assistant response");
    assert!(assistant.is_assistant());
}