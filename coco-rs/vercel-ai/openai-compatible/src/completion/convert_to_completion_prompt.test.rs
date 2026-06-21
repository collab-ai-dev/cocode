use super::*;
use serde_json::json;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;

#[test]
fn converts_simple_prompt_with_structured_format() {
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
fn converts_multi_turn_conversation() {
    let prompt = vec![
        LanguageModelV4Message::system("You are a helpful bot"),
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
                text: "And 3+3?".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
    ];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(
        result.prompt,
        "You are a helpful bot\n\nuser:\nWhat is 2+2?\n\nassistant:\n4\n\nuser:\nAnd 3+3?\n\nassistant:\n"
    );
}

#[test]
fn converts_without_system_message() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::Text(TextPart {
            text: "Hello".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(result.prompt, "user:\nHello\n\nassistant:\n");
    assert_eq!(result.stop_sequences, vec!["\nuser:"]);
}

#[test]
fn returns_stop_sequences() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::Text(TextPart {
            text: "Hi".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let result = convert_to_completion_prompt(&prompt).unwrap();
    assert_eq!(result.stop_sequences, vec!["\nuser:"]);
}

#[test]
fn errors_on_tool_call_in_assistant_message() {
    let prompt = vec![
        LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart {
                text: "Hello".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::ToolCall(ToolCallPart::new(
                "call_1",
                "my_tool",
                json!({"arg": "value"}),
            ))],
            provider_options: None,
        },
    ];
    let result = convert_to_completion_prompt(&prompt);
    match result {
        Err(e) => assert!(
            e.to_string().contains("tool-call"),
            "Error should mention tool-call: {e}",
        ),
        Ok(_) => panic!("Expected error for tool-call in assistant message"),
    }
}
