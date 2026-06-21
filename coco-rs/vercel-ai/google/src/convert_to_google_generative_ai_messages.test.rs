use super::*;
use vercel_ai_provider::FilePart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;
use vercel_ai_provider::UserContentPart;

#[test]
fn converts_system_message() {
    let messages = vec![LanguageModelV4Message::system("You are helpful.")];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert!(result.system_instruction.is_some());
    let si = result.system_instruction.unwrap();
    assert_eq!(si.parts.len(), 1);
    assert_eq!(si.parts[0].text, "You are helpful.");
    assert!(result.contents.is_empty());
}

#[test]
fn converts_developer_message_to_system_instruction() {
    let messages = vec![LanguageModelV4Message::developer_text("Follow app policy.")];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    let si = result.system_instruction.unwrap();
    assert_eq!(si.parts[0].text, "Follow app policy.");
    assert!(result.contents.is_empty());
}

#[test]
fn system_non_text_part_produces_warning() {
    let messages = vec![LanguageModelV4Message::system_parts(vec![
        UserContentPart::text("text"),
        UserContentPart::File(FilePart::image(vec![1, 2, 3], "image/png")),
    ])];
    let (prompt, warnings) = convert_to_google_generative_ai_messages_with_warnings(
        &messages,
        &ConvertOptions::default(),
    )
    .unwrap();
    assert_eq!(
        prompt.system_instruction.unwrap().parts[0].text,
        "text".to_string()
    );
    assert_eq!(warnings.len(), 1);
}

#[test]
fn converts_system_as_prepend_when_unsupported() {
    let messages = vec![
        LanguageModelV4Message::system("You are helpful."),
        LanguageModelV4Message::system("Be concise."),
        LanguageModelV4Message::user_text("Hello"),
    ];
    let options = ConvertOptions {
        supports_system_instruction: false,
        ..ConvertOptions::default()
    };
    let result = convert_to_google_generative_ai_messages(&messages, &options).unwrap();
    assert!(result.system_instruction.is_none());
    // System text prepended to first user message, user text follows
    assert_eq!(result.contents.len(), 1);
    assert_eq!(result.contents[0].role, GoogleContentRole::User);
    assert_eq!(result.contents[0].parts.len(), 2);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text { text, .. } => {
            assert_eq!(text, "You are helpful.\n\nBe concise.\n\n");
        }
        _ => panic!("Expected text part"),
    }
    match &result.contents[0].parts[1] {
        GoogleGenerativeAIContentPart::Text { text, .. } => {
            assert_eq!(text, "Hello");
        }
        _ => panic!("Expected text part"),
    }
}

#[test]
fn rejects_system_after_user() {
    let messages = vec![
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::system("Too late"),
    ];
    let result = convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("system messages are only supported at the beginning")
    );
}

#[test]
fn converts_user_text_message() {
    let messages = vec![LanguageModelV4Message::user_text("Hello")];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert!(result.system_instruction.is_none());
    assert_eq!(result.contents.len(), 1);
    assert_eq!(result.contents[0].role, GoogleContentRole::User);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text { text, .. } => assert_eq!(text, "Hello"),
        _ => panic!("Expected text part"),
    }
}

#[test]
fn converts_assistant_text_message() {
    let messages = vec![LanguageModelV4Message::assistant_text("Hi there")];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents.len(), 1);
    assert_eq!(result.contents[0].role, GoogleContentRole::Model);
}

#[test]
fn converts_tool_call_in_assistant_message() {
    let parts = vec![AssistantContentPart::ToolCall(ToolCallPart::new(
        "call_1",
        "get_weather",
        serde_json::json!({"location": "NYC"}),
    ))];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents.len(), 1);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::FunctionCall { function_call, .. } => {
            assert_eq!(function_call.name, "get_weather");
        }
        _ => panic!("Expected function call part"),
    }
}

#[test]
fn converts_tool_result_with_name_content_format() {
    let parts = vec![ToolContentPart::ToolResult(ToolResultPart::new(
        "call_1",
        "get_weather",
        ToolResultContent::text("Sunny, 72F"),
    ))];
    let messages = vec![LanguageModelV4Message::tool(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents.len(), 1);
    assert_eq!(result.contents[0].role, GoogleContentRole::User);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::FunctionResponse {
            function_response, ..
        } => {
            assert_eq!(function_response.name, "get_weather");
            assert_eq!(function_response.response["name"], "get_weather");
            assert_eq!(function_response.response["content"], "Sunny, 72F");
        }
        _ => panic!("Expected function response part"),
    }
}

#[test]
fn converts_tool_result_execution_denied() {
    let parts = vec![ToolContentPart::ToolResult(ToolResultPart::new(
        "call_1",
        "dangerous_tool",
        ToolResultContent::execution_denied(Some("Not allowed".to_string())),
    ))];
    let messages = vec![LanguageModelV4Message::tool(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::FunctionResponse {
            function_response, ..
        } => {
            assert_eq!(function_response.name, "dangerous_tool");
            assert_eq!(function_response.response["name"], "dangerous_tool");
            assert_eq!(function_response.response["content"], "Not allowed");
        }
        _ => panic!("Expected function response part"),
    }
}

#[test]
fn converts_tool_result_execution_denied_default_reason() {
    let parts = vec![ToolContentPart::ToolResult(ToolResultPart::new(
        "call_1",
        "my_tool",
        ToolResultContent::execution_denied(None::<String>),
    ))];
    let messages = vec![LanguageModelV4Message::tool(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::FunctionResponse {
            function_response, ..
        } => {
            assert_eq!(
                function_response.response["content"],
                "Tool execution denied."
            );
        }
        _ => panic!("Expected function response part"),
    }
}

#[test]
fn converts_tool_result_content_with_image() {
    let parts = vec![ToolContentPart::ToolResult(ToolResultPart::new(
        "call_1",
        "image_tool",
        ToolResultContent::content_parts(vec![ToolResultContentPart::FileData {
            data: "aGVsbG8=".to_string(),
            media_type: "image/png".to_string(),
            filename: None,
            provider_options: None,
        }]),
    ))];
    let messages = vec![LanguageModelV4Message::tool(parts)];
    let opts = ConvertOptions {
        supports_function_response_parts: false,
        ..ConvertOptions::default()
    };
    let result = convert_to_google_generative_ai_messages(&messages, &opts).unwrap();
    // Legacy mode: should produce inlineData + text description
    assert_eq!(result.contents[0].parts.len(), 2);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::InlineData { inline_data, .. } => {
            assert_eq!(inline_data.mime_type, "image/png");
            assert_eq!(inline_data.data, "aGVsbG8=");
        }
        _ => panic!("Expected inline data part"),
    }
    match &result.contents[0].parts[1] {
        GoogleGenerativeAIContentPart::Text { text, .. } => {
            assert_eq!(
                text,
                "Tool executed successfully and returned this image as a response"
            );
        }
        _ => panic!("Expected text part"),
    }
}

#[test]
fn converts_tool_result_content_with_text() {
    let parts = vec![ToolContentPart::ToolResult(ToolResultPart::new(
        "call_1",
        "text_tool",
        ToolResultContent::content_parts(vec![ToolResultContentPart::Text {
            text: "result text".to_string(),
            provider_options: None,
        }]),
    ))];
    let messages = vec![LanguageModelV4Message::tool(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::FunctionResponse {
            function_response, ..
        } => {
            assert_eq!(function_response.name, "text_tool");
            assert_eq!(function_response.response["name"], "text_tool");
            assert_eq!(function_response.response["content"], "result text");
        }
        _ => panic!("Expected function response part"),
    }
}

#[test]
fn converts_file_with_base64() {
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::SharedV4FileData;

    let parts = vec![UserContentPart::File(FilePart::new(
        SharedV4FileData::data_base64("aGVsbG8="),
        "image/png",
    ))];
    let messages = vec![LanguageModelV4Message::user(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::InlineData { inline_data, .. } => {
            assert_eq!(inline_data.mime_type, "image/png");
            assert_eq!(inline_data.data, "aGVsbG8=");
        }
        _ => panic!("Expected inline data part"),
    }
}

#[test]
fn converts_image_wildcard_to_jpeg() {
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::SharedV4FileData;

    let parts = vec![UserContentPart::File(FilePart::new(
        SharedV4FileData::data_base64("aGVsbG8="),
        "image/*",
    ))];
    let messages = vec![LanguageModelV4Message::user(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::InlineData { inline_data, .. } => {
            assert_eq!(inline_data.mime_type, "image/jpeg");
        }
        _ => panic!("Expected inline data part"),
    }
}

#[test]
fn sends_reasoning_parts_as_thought_text() {
    let parts = vec![
        AssistantContentPart::reasoning("thinking..."),
        AssistantContentPart::text("answer"),
    ];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 2);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text {
            text,
            thought,
            thought_signature,
        } => {
            assert_eq!(text, "thinking...");
            assert_eq!(*thought, Some(true));
            assert!(thought_signature.is_none());
        }
        _ => panic!("Expected text part with thought=true"),
    }
    match &result.contents[0].parts[1] {
        GoogleGenerativeAIContentPart::Text { text, thought, .. } => {
            assert_eq!(text, "answer");
            assert!(thought.is_none());
        }
        _ => panic!("Expected text part"),
    }
}

#[test]
fn filters_empty_text_parts() {
    let parts = vec![
        AssistantContentPart::text(""),
        AssistantContentPart::text("visible"),
    ];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 1);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text { text, .. } => assert_eq!(text, "visible"),
        _ => panic!("Expected text part"),
    }
}

#[test]
fn filters_empty_reasoning_parts() {
    let parts = vec![
        AssistantContentPart::reasoning(""),
        AssistantContentPart::reasoning("thinking"),
    ];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 1);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text { text, thought, .. } => {
            assert_eq!(text, "thinking");
            assert_eq!(*thought, Some(true));
        }
        _ => panic!("Expected text part"),
    }
}

#[test]
fn converts_reasoning_file_with_thought_and_signature() {
    use vercel_ai_provider::ReasoningFilePart;

    let meta = vercel_ai_provider::ProviderMetadata::from_map(
        [(
            "google".to_string(),
            serde_json::json!({"thoughtSignature": "sig_rf"}),
        )]
        .into_iter()
        .collect(),
    );

    let parts = vec![AssistantContentPart::ReasoningFile(
        ReasoningFilePart::from_base64("AAECAw==", "image/png").with_metadata(meta),
    )];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 1);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::InlineData {
            inline_data,
            thought,
            thought_signature,
        } => {
            assert_eq!(inline_data.mime_type, "image/png");
            assert_eq!(inline_data.data, "AAECAw==");
            assert_eq!(*thought, Some(true));
            assert_eq!(thought_signature.as_deref(), Some("sig_rf"));
        }
        _ => panic!("Expected inline data part"),
    }
}

#[test]
fn converts_reasoning_file_without_signature() {
    use vercel_ai_provider::ReasoningFilePart;

    let parts = vec![AssistantContentPart::ReasoningFile(
        ReasoningFilePart::from_base64("BAUG", "image/jpeg"),
    )];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 1);
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::InlineData {
            inline_data,
            thought,
            thought_signature,
        } => {
            assert_eq!(inline_data.mime_type, "image/jpeg");
            assert_eq!(inline_data.data, "BAUG");
            assert_eq!(*thought, Some(true));
            assert!(thought_signature.is_none());
        }
        _ => panic!("Expected inline data part"),
    }
}

#[test]
fn rejects_url_file_data_in_reasoning_file() {
    use vercel_ai_provider::ReasoningFilePart;

    let parts = vec![AssistantContentPart::ReasoningFile(
        ReasoningFilePart::from_url("https://example.com/image.png", "image/png"),
    )];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result = convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("File data URLs in assistant messages are not supported")
    );
}

#[test]
fn rejects_url_file_data_in_assistant_file() {
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::SharedV4FileData;

    let parts = vec![AssistantContentPart::File(FilePart::new(
        SharedV4FileData::url("https://example.com/image.png"),
        "image/png",
    ))];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result = convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("File data URLs in assistant messages are not supported")
    );
}

#[test]
fn handles_mixed_reasoning_reasoning_file_text_and_tool_call() {
    use vercel_ai_provider::ReasoningFilePart;

    let meta1 = vercel_ai_provider::ProviderMetadata::from_map(
        [(
            "google".to_string(),
            serde_json::json!({"thoughtSignature": "sig1"}),
        )]
        .into_iter()
        .collect(),
    );

    let meta2 = vercel_ai_provider::ProviderMetadata::from_map(
        [(
            "google".to_string(),
            serde_json::json!({"thoughtSignature": "sig2"}),
        )]
        .into_iter()
        .collect(),
    );

    let parts = vec![
        AssistantContentPart::Reasoning(
            vercel_ai_provider::content::ReasoningPart::new("Thinking about this...")
                .with_metadata(meta1),
        ),
        AssistantContentPart::ReasoningFile(
            ReasoningFilePart::from_base64("AAECAw==", "image/png").with_metadata(meta2),
        ),
        AssistantContentPart::text("Here is the result"),
        AssistantContentPart::tool_call("call_1", "search", serde_json::json!({"q": "test"})),
    ];
    let messages = vec![LanguageModelV4Message::assistant(parts)];
    let result =
        convert_to_google_generative_ai_messages(&messages, &ConvertOptions::default()).unwrap();
    assert_eq!(result.contents[0].parts.len(), 4);

    // 1. Reasoning text with thought=true
    match &result.contents[0].parts[0] {
        GoogleGenerativeAIContentPart::Text {
            text,
            thought,
            thought_signature,
        } => {
            assert_eq!(text, "Thinking about this...");
            assert_eq!(*thought, Some(true));
            assert_eq!(thought_signature.as_deref(), Some("sig1"));
        }
        _ => panic!("Expected reasoning text part"),
    }

    // 2. Reasoning file with thought=true
    match &result.contents[0].parts[1] {
        GoogleGenerativeAIContentPart::InlineData {
            inline_data,
            thought,
            thought_signature,
        } => {
            assert_eq!(inline_data.data, "AAECAw==");
            assert_eq!(*thought, Some(true));
            assert_eq!(thought_signature.as_deref(), Some("sig2"));
        }
        _ => panic!("Expected inline data part"),
    }

    // 3. Regular text
    match &result.contents[0].parts[2] {
        GoogleGenerativeAIContentPart::Text { text, thought, .. } => {
            assert_eq!(text, "Here is the result");
            assert!(thought.is_none());
        }
        _ => panic!("Expected text part"),
    }

    // 4. Function call
    match &result.contents[0].parts[3] {
        GoogleGenerativeAIContentPart::FunctionCall { function_call, .. } => {
            assert_eq!(function_call.name, "search");
        }
        _ => panic!("Expected function call part"),
    }
}
