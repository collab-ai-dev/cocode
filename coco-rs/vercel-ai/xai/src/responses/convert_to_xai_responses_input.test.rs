use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultPart;

fn xai_meta(inner: serde_json::Value) -> ProviderMetadata {
    let mut m = HashMap::new();
    m.insert("xai".to_string(), inner);
    ProviderMetadata(m)
}

#[test]
fn system_message_becomes_system_item() {
    let prompt = vec![LanguageModelV4Message::system("be terse")];
    let (input, warnings) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({ "role": "system", "content": "be terse" })]
    );
    assert!(warnings.is_empty());
}

#[test]
fn user_text_and_image_url() {
    let prompt = vec![LanguageModelV4Message::user(vec![
        UserContentPart::text("look:"),
        UserContentPart::image_url("https://example.com/a.png", "image/png"),
    ])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({
            "role": "user",
            "content": [
                { "type": "input_text", "text": "look:" },
                { "type": "input_image", "image_url": "https://example.com/a.png" },
            ]
        })]
    );
}

#[test]
fn image_detail_is_forwarded() {
    let file = FilePart {
        data: SharedV4FileData::url("https://example.com/a.png"),
        media_type: "image/png".into(),
        filename: None,
        provider_metadata: Some(xai_meta(json!({ "imageDetail": "high" }))),
    };
    let prompt = vec![LanguageModelV4Message::user(vec![UserContentPart::File(
        file,
    )])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(input[0]["content"][0]["detail"], "high");
}

#[test]
fn non_image_url_becomes_input_file() {
    let file = FilePart {
        data: SharedV4FileData::url("https://example.com/doc.pdf"),
        media_type: "application/pdf".into(),
        filename: None,
        provider_metadata: None,
    };
    let prompt = vec![LanguageModelV4Message::user(vec![UserContentPart::File(
        file,
    )])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input[0]["content"][0],
        json!({ "type": "input_file", "file_url": "https://example.com/doc.pdf" })
    );
}

#[test]
fn non_image_inline_data_errors() {
    let file = FilePart::new(SharedV4FileData::data_bytes(vec![0]), "application/pdf");
    let prompt = vec![LanguageModelV4Message::user(vec![UserContentPart::File(
        file,
    )])];
    assert!(convert_to_xai_responses_input(&prompt).is_err());
}

#[test]
fn file_reference_becomes_input_file_with_id() {
    let mut reference = HashMap::new();
    reference.insert("xai".to_string(), "file-123".to_string());
    let file = FilePart {
        data: SharedV4FileData::Reference { reference },
        media_type: "application/pdf".into(),
        filename: None,
        provider_metadata: None,
    };
    let prompt = vec![LanguageModelV4Message::user(vec![UserContentPart::File(
        file,
    )])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input[0]["content"][0],
        json!({ "type": "input_file", "file_id": "file-123" })
    );
}

#[test]
fn assistant_text_carries_item_id() {
    let part = AssistantContentPart::Text(
        TextPart::new("answer").with_metadata(xai_meta(json!({ "itemId": "msg_1" }))),
    );
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({ "role": "assistant", "content": "answer", "id": "msg_1" })]
    );
}

#[test]
fn assistant_tool_call_becomes_function_call() {
    let part = AssistantContentPart::ToolCall(ToolCallPart::new(
        "call_1",
        "get_weather",
        json!({ "city": "SF" }),
    ));
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({
            "type": "function_call",
            "id": "call_1",
            "call_id": "call_1",
            "name": "get_weather",
            "arguments": "{\"city\":\"SF\"}",
        })]
    );
}

#[test]
fn provider_executed_tool_call_is_skipped() {
    let part = AssistantContentPart::ToolCall(
        ToolCallPart::new("call_1", "web_search", json!({})).with_provider_executed(true),
    );
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert!(input.is_empty());
}

#[test]
fn reasoning_round_trips_with_item_id() {
    let part = AssistantContentPart::Reasoning(
        ReasoningPart::new("thinking").with_metadata(xai_meta(json!({ "itemId": "rs_1" }))),
    );
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, warnings) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{ "type": "summary_text", "text": "thinking" }],
        })]
    );
    assert!(warnings.is_empty());
}

#[test]
fn reasoning_without_metadata_warns_and_drops() {
    let part = AssistantContentPart::Reasoning(ReasoningPart::new("thinking"));
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, warnings) = convert_to_xai_responses_input(&prompt).unwrap();
    assert!(input.is_empty());
    assert_eq!(warnings.len(), 1);
}

#[test]
fn reasoning_with_encrypted_content() {
    let part =
        AssistantContentPart::Reasoning(ReasoningPart::new("").with_metadata(xai_meta(json!({
            "itemId": "rs_1",
            "reasoningEncryptedContent": "enc",
        }))));
    let prompt = vec![LanguageModelV4Message::assistant(vec![part])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(input[0]["encrypted_content"], "enc");
    assert_eq!(input[0]["summary"], json!([]));
}

#[test]
fn tool_result_becomes_function_call_output() {
    let prompt = vec![LanguageModelV4Message::tool(vec![
        ToolContentPart::ToolResult(ToolResultPart::new(
            "call_1",
            "get_weather",
            ToolResultContent::text("sunny"),
        )),
    ])];
    let (input, _) = convert_to_xai_responses_input(&prompt).unwrap();
    assert_eq!(
        input,
        vec![json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "sunny",
        })]
    );
}
