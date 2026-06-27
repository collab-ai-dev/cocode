use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;

use super::*;

#[test]
fn test_convert_system_message_passthrough() {
    let system = vec![LanguageModelV4Message::system("You are helpful")];
    let messages = vec![LanguageModelV4Message::user_text("Hello")];

    let result = convert_to_language_model_prompt(Some(system), messages).unwrap();
    assert_eq!(result.len(), 2);
    assert!(matches!(result[0], LanguageModelV4Message::System { .. }));
    assert!(matches!(result[1], LanguageModelV4Message::User { .. }));
}

#[test]
fn test_convert_user_text_message() {
    let messages = vec![LanguageModelV4Message::user_text("Hello")];
    let result = convert_to_language_model_prompt(None, messages).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], LanguageModelV4Message::User { .. }));
}

#[test]
fn test_convert_assistant_message() {
    let messages = vec![
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::text("Hi there")],
            provider_options: None,
        },
    ];
    let result = convert_to_language_model_prompt(None, messages).unwrap();
    assert_eq!(result.len(), 2);
    assert!(matches!(
        result[1],
        LanguageModelV4Message::Assistant { .. }
    ));
}

#[test]
fn test_convert_tool_result_clears_pending() {
    let messages = vec![
        LanguageModelV4Message::user_text("Do something"),
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::tool_call(
                "tc1",
                "my_tool",
                serde_json::json!({}),
            )],
            provider_options: None,
        },
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "tc1",
                "my_tool",
                ToolResultContent::text("result"),
            ))],
            provider_options: None,
        },
        LanguageModelV4Message::user_text("Thanks"),
    ];
    let result = convert_to_language_model_prompt(None, messages).unwrap();
    assert_eq!(result.len(), 4);
}

#[test]
fn test_convert_missing_tool_results_before_user() {
    let messages = vec![
        LanguageModelV4Message::user_text("Do something"),
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::tool_call(
                "tc1",
                "my_tool",
                serde_json::json!({}),
            )],
            provider_options: None,
        },
        // Missing tool result!
        LanguageModelV4Message::user_text("Continue"),
    ];
    let result = convert_to_language_model_prompt(None, messages);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.tool_call_ids.contains(&"tc1".to_string()));
}

#[test]
fn test_convert_missing_tool_results_at_end() {
    let messages = vec![
        LanguageModelV4Message::user_text("Do something"),
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::tool_call(
                "tc1",
                "my_tool",
                serde_json::json!({}),
            )],
            provider_options: None,
        },
        // Missing tool result at end
    ];
    let result = convert_to_language_model_prompt(None, messages);
    assert!(result.is_err());
}

#[test]
fn test_convert_no_system() {
    let messages = vec![LanguageModelV4Message::user_text("Hello")];
    let result = convert_to_language_model_prompt(None, messages).unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn test_combine_consecutive_tool_messages() {
    let messages = vec![
        LanguageModelV4Message::user_text("Do something"),
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "tc1",
                "tool1",
                ToolResultContent::text("result1"),
            ))],
            provider_options: None,
        },
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "tc2",
                "tool2",
                ToolResultContent::text("result2"),
            ))],
            provider_options: None,
        },
    ];
    let combined = combine_tool_messages(messages);
    assert_eq!(combined.len(), 2); // user + combined tool
    if let LanguageModelV4Message::Tool { content, .. } = &combined[1] {
        assert_eq!(content.len(), 2);
    } else {
        panic!("Expected Tool message");
    }
}

#[test]
fn test_combine_non_consecutive_tool_messages() {
    let messages = vec![
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "tc1",
                "tool1",
                ToolResultContent::text("result1"),
            ))],
            provider_options: None,
        },
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "tc2",
                "tool2",
                ToolResultContent::text("result2"),
            ))],
            provider_options: None,
        },
    ];
    let combined = combine_tool_messages(messages);
    assert_eq!(combined.len(), 3); // Not combined since non-consecutive
}
