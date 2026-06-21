//! Tests for prune_messages.rs

use super::*;

#[test]
fn test_prune_reasoning_all() {
    let messages = vec![
        LanguageModelV4Message::assistant_text("Hello"),
        LanguageModelV4Message::assistant_text("World"),
    ];

    let result = prune_reasoning(messages, ReasoningPruneMode::All);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_prune_empty_messages() {
    let messages = vec![
        LanguageModelV4Message::assistant_text("Hello"),
        LanguageModelV4Message::Assistant {
            content: vec![],
            provider_options: None,
        },
    ];

    let options = PruneMessagesOptions {
        remove_empty: true,
        ..Default::default()
    };

    let result = prune_messages(messages, &options);
    assert_eq!(result.len(), 1);
}
