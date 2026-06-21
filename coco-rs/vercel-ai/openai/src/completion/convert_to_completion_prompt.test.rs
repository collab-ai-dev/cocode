use super::*;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;

#[test]
fn system_then_user_formats_with_role_prefixes() {
    let prompt = vec![
        LanguageModelV4Message::system("Be helpful"),
        LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart {
                text: "Hello".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
    ];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(result.prompt, "Be helpful\n\nuser:\nHello\n\nassistant:\n");
    assert_eq!(result.stop_sequences, vec!["\nuser:"]);
}

#[test]
fn user_only_no_system() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::Text(TextPart {
            text: "Hi".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(result.prompt, "user:\nHi\n\nassistant:\n");
}

#[test]
fn multi_turn_conversation() {
    let prompt = vec![
        LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart {
                text: "What is 2+2?".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "4".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart {
                text: "Thanks".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
    ];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(
        result.prompt,
        "user:\nWhat is 2+2?\n\nassistant:\n4\n\nuser:\nThanks\n\nassistant:\n"
    );
}

#[test]
fn system_after_first_message_is_error() {
    let prompt = vec![
        LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart {
                text: "Hi".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::system("Bad system"),
    ];
    let result = convert_to_completion_prompt(&prompt);
    assert!(result.is_err());
}

#[test]
fn tool_message_is_error() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![],
        provider_options: None,
    }];
    let result = convert_to_completion_prompt(&prompt);
    assert!(result.is_err());
}

#[test]
fn assistant_tool_call_is_error() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc1".into(),
            tool_name: "test".into(),
            input: serde_json::json!({}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];
    let result = convert_to_completion_prompt(&prompt);
    assert!(result.is_err());
}
