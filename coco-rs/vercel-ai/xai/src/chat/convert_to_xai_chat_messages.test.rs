use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;
use vercel_ai_provider::UserContentPart;

#[test]
fn system_message_collapses_to_string() {
    let prompt = vec![LanguageModelV4Message::system("be terse")];
    let (messages, warnings) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(
        messages,
        vec![json!({"role": "system", "content": "be terse"})]
    );
    assert!(warnings.is_empty());
}

#[test]
fn single_text_user_message_is_plain_string() {
    let prompt = vec![LanguageModelV4Message::user_text("hello")];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(messages, vec![json!({"role": "user", "content": "hello"})]);
}

#[test]
fn multipart_user_message_with_image_url() {
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("look:"),
        UserContentPart::image_url("https://example.com/a.png", "image/png"),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(
        messages,
        vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look:"},
                {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}},
            ]
        })]
    );
}

#[test]
fn image_bytes_become_data_uri() {
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("x"),
        UserContentPart::image(vec![1, 2, 3], "image/png"),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    let url = messages[0]["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert!(url.starts_with("data:image/png;base64,"));
}

#[test]
fn image_detail_provider_option_is_forwarded() {
    use std::collections::HashMap;
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::ProviderMetadata;
    use vercel_ai_provider::SharedV4FileData;

    let mut meta = HashMap::new();
    meta.insert(
        "xai".to_string(),
        serde_json::json!({ "imageDetail": "low" }),
    );
    let file = FilePart {
        data: SharedV4FileData::url("https://example.com/a.png"),
        media_type: "image/png".into(),
        filename: None,
        provider_metadata: Some(ProviderMetadata(meta)),
    };
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("x"),
        UserContentPart::File(file),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(messages[0]["content"][1]["image_url"]["detail"], "low");
}

#[test]
fn invalid_image_detail_is_dropped() {
    use std::collections::HashMap;
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::ProviderMetadata;
    use vercel_ai_provider::SharedV4FileData;

    let mut meta = HashMap::new();
    meta.insert(
        "xai".to_string(),
        serde_json::json!({ "imageDetail": "ultra" }),
    );
    let file = FilePart {
        data: SharedV4FileData::url("https://example.com/a.png"),
        media_type: "image/png".into(),
        filename: None,
        provider_metadata: Some(ProviderMetadata(meta)),
    };
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("x"),
        UserContentPart::File(file),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert!(
        messages[0]["content"][1]["image_url"]
            .get("detail")
            .is_none()
    );
}

#[test]
fn non_image_file_part_errors() {
    use vercel_ai_provider::FilePart;
    use vercel_ai_provider::SharedV4FileData;
    let file = FilePart::new(SharedV4FileData::data_bytes(vec![0]), "application/pdf");
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("x"),
        UserContentPart::File(file),
    ])];
    assert!(convert_to_xai_chat_messages(&prompt).is_err());
}

#[test]
fn assistant_message_drops_reasoning_keeps_tool_call() {
    let prompt = vec![LanguageModelV4Message::assistant(vec![
        AssistantContentPart::Reasoning(ReasoningPart::new("thinking")),
        AssistantContentPart::Text(TextPart::new("answer")),
        AssistantContentPart::ToolCall(ToolCallPart::new(
            "call_1",
            "get_weather",
            json!({"city": "SF"}),
        )),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    // xAI chat assistant messages carry no reasoning field.
    assert_eq!(
        messages,
        vec![json!({
            "role": "assistant",
            "content": "answer",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}
            }]
        })]
    );
}

#[test]
fn tool_result_text_output() {
    let prompt = vec![LanguageModelV4Message::tool(vec![
        ToolContentPart::ToolResult(ToolResultPart::new(
            "call_1",
            "get_weather",
            ToolResultContent::text("sunny"),
        )),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(
        messages,
        vec![json!({"role": "tool", "tool_call_id": "call_1", "content": "sunny"})]
    );
}

#[test]
fn tool_result_json_output_is_stringified() {
    let prompt = vec![LanguageModelV4Message::tool(vec![
        ToolContentPart::ToolResult(ToolResultPart::new(
            "c",
            "t",
            ToolResultContent::json(json!({"ok": true})),
        )),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(messages[0]["content"].as_str().unwrap(), "{\"ok\":true}");
}

#[test]
fn multimodal_tool_result_degrades_gracefully() {
    use vercel_ai_provider::ToolResultContentPart;
    let content = ToolResultContent::content_parts(vec![
        ToolResultContentPart::text("here is the chart"),
        ToolResultContentPart::file_data("aGVsbG8=", "image/png"),
    ]);
    let prompt = vec![LanguageModelV4Message::tool(vec![
        ToolContentPart::ToolResult(ToolResultPart::new("c", "t", content)),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    let text = messages[0]["content"].as_str().unwrap();
    assert!(text.contains("here is the chart"));
    assert!(text.contains("image/png content omitted"));
    // The base64 blob must NOT leak into the prompt.
    assert!(!text.contains("aGVsbG8="));
}

#[test]
fn execution_denied_uses_default_reason() {
    let prompt = vec![LanguageModelV4Message::tool(vec![
        ToolContentPart::ToolResult(ToolResultPart::new(
            "c",
            "t",
            ToolResultContent::execution_denied(None),
        )),
    ])];
    let (messages, _) = convert_to_xai_chat_messages(&prompt).unwrap();
    assert_eq!(
        messages[0]["content"].as_str().unwrap(),
        "Tool call execution denied."
    );
}
