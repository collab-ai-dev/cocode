use vercel_ai_provider::LanguageModelV4Message;

use super::*;

#[test]
fn test_standardize_text_prompt() {
    let result = standardize_text_prompt("Hello");
    assert!(result.system.is_none());
    assert_eq!(result.messages.len(), 1);
    assert!(matches!(
        result.messages[0],
        LanguageModelV4Message::User { .. }
    ));
}

#[test]
fn test_standardize_prompt_text_with_system() {
    let result = standardize_prompt(
        Some("Hello".to_string()),
        None,
        Some("Be helpful".to_string()),
    )
    .unwrap();
    assert!(result.system.is_some());
    let system = result.system.unwrap();
    assert_eq!(system.len(), 1);
    assert!(matches!(system[0], LanguageModelV4Message::System { .. }));
    assert_eq!(result.messages.len(), 1);
}

#[test]
fn test_standardize_prompt_messages_passthrough() {
    let messages = vec![
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::user_text("World"),
    ];
    let result = standardize_prompt(None, Some(messages), None).unwrap();
    assert!(result.system.is_none());
    assert_eq!(result.messages.len(), 2);
}

#[test]
fn test_standardize_prompt_empty_messages_error() {
    let result = standardize_prompt(None, Some(vec![]), None);
    assert!(result.is_err());
}

#[test]
fn test_standardize_prompt_both_provided_error() {
    let messages = vec![LanguageModelV4Message::user_text("Hello")];
    let result = standardize_prompt(Some("Hello".to_string()), Some(messages), None);
    assert!(result.is_err());
}

#[test]
fn test_standardize_prompt_neither_provided_error() {
    let result = standardize_prompt(None, None, None);
    assert!(result.is_err());
}
