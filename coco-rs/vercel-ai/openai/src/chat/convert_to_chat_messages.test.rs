use super::*;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolResultPart;

fn system_msg(content: &str) -> LanguageModelV4Message {
    LanguageModelV4Message::system(content)
}

fn user_text(text: &str) -> LanguageModelV4Message {
    LanguageModelV4Message::User {
        content: vec![UserContentPart::Text(TextPart {
            text: text.into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }
}

fn assistant_text(text: &str) -> LanguageModelV4Message {
    LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Text(TextPart {
            text: text.into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }
}

#[test]
fn converts_system_message_as_system() {
    let prompt = vec![system_msg("You are helpful")];
    let (msgs, warnings) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    assert!(warnings.is_empty());
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "You are helpful");
}

#[test]
fn converts_system_message_as_developer() {
    let prompt = vec![system_msg("You are helpful")];
    let (msgs, warnings) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::Developer);
    assert!(warnings.is_empty());
    assert_eq!(msgs[0]["role"], "developer");
}

#[test]
fn removes_system_message_with_warning() {
    let prompt = vec![system_msg("You are helpful")];
    let (msgs, warnings) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::Remove);
    assert!(msgs.is_empty());
    assert_eq!(warnings.len(), 1);
    assert!(matches!(
        warnings[0],
        Warning::Other { ref message } if message == "system messages are removed for this model"
    ));
}

#[test]
fn converts_user_text_message() {
    let prompt = vec![user_text("Hello")];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "Hello");
}

#[test]
fn converts_assistant_text_message() {
    let prompt = vec![assistant_text("Hi there")];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    assert_eq!(msgs[0]["role"], "assistant");
    assert_eq!(msgs[0]["content"], "Hi there");
}

#[test]
fn converts_assistant_tool_call() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_123".into(),
            tool_name: "get_weather".into(),
            input: serde_json::json!({"city": "SF"}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    assert_eq!(msgs[0]["role"], "assistant");
    let tc = &msgs[0]["tool_calls"][0];
    assert_eq!(tc["id"], "call_123");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather");
}

#[test]
fn converts_tool_result() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_123".into(),
            tool_name: "get_weather".into(),
            output: ToolResultContent::Text {
                value: "72F and sunny".into(),
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    assert_eq!(msgs[0]["role"], "tool");
    assert_eq!(msgs[0]["tool_call_id"], "call_123");
    assert_eq!(msgs[0]["content"], "72F and sunny");
}

#[test]
fn tool_result_content_image_degrades_to_marker() {
    // OpenAI Chat Completions can only carry a single string in the
    // `tool` role message — image content cannot pass through. The
    // serializer must replace FileData with a visible marker so the
    // model knows *something* was there. Pre-refactor this branch
    // silently dropped non-Text parts.
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_img".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![ToolResultContentPart::FileData {
                    data: "iVBOR...".into(),
                    media_type: "image/png".into(),
                    filename: None,
                    provider_options: None,
                }],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    let content = msgs[0]["content"].as_str().unwrap();
    assert!(content.contains("image/png"), "got: {content}");
    assert!(
        content.contains("doesn't support multimodal"),
        "expected degradation marker, got: {content}"
    );
}

#[test]
fn tool_result_content_mixed_text_and_image_keeps_text_drops_image() {
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_mix".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![
                    ToolResultContentPart::Text {
                        text: "explanation".into(),
                        provider_options: None,
                    },
                    ToolResultContentPart::FileData {
                        data: "iVBOR...".into(),
                        media_type: "image/png".into(),
                        filename: None,
                        provider_options: None,
                    },
                ],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (msgs, _) = convert_to_openai_chat_messages(&prompt, SystemMessageMode::System);
    let content = msgs[0]["content"].as_str().unwrap();
    assert!(content.contains("explanation"), "got: {content}");
    assert!(
        content.contains("image/png"),
        "expected marker for image, got: {content}"
    );
}
