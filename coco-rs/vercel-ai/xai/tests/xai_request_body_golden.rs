//! Request-body goldens for the xAI chat + Responses models: `get_args` with a
//! rich option set is snapshotted as JSON, so any silent change in the wire
//! request shape (a renamed field, a dropped param, a moved namespace) shows up
//! as a snapshot diff. The response-side counterpart lives in the cassette
//! replay harnesses.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;
use vercel_ai_xai::XaiProviderSettings;
use vercel_ai_xai::create_xai;

fn xai_provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("xai".to_string(), ns);
    ProviderOptions(map)
}

fn weather_tool() -> LanguageModelV4Tool {
    LanguageModelV4Tool::Function(LanguageModelV4FunctionTool {
        name: "get_weather".into(),
        description: Some("Get weather".into()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "city": { "type": "string" } }
        }),
        input_examples: None,
        strict: Some(true),
        provider_options: None,
    })
}

/// Golden of the full chat request body: sampling params, seed,
/// `max_completion_tokens`, strict json_schema response format, the `xai`
/// provider-option namespace (reasoningEffort / logprobs / topLogprobs /
/// parallel_function_calling), a sanitized function tool, and tool_choice.
#[test]
fn chat_request_body_golden() {
    let model = create_xai(XaiProviderSettings {
        base_url: Some("https://api.x.ai/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .chat("grok-4.5");

    let options = LanguageModelV4CallOptions {
        prompt: vec![
            LanguageModelV4Message::system("Be brief."),
            LanguageModelV4Message::user_text("weather in SF?"),
        ],
        max_output_tokens: Some(256),
        temperature: Some(0.5),
        top_p: Some(0.9),
        seed: Some(42),
        response_format: Some(ResponseFormat::Json {
            schema: Some(json!({
                "type": "object",
                "properties": { "answer": { "type": "string" } }
            })),
            name: Some("weather_report".to_string()),
            description: None,
        }),
        provider_options: Some(xai_provider_options(json!({
            "reasoningEffort": "high",
            "logprobs": true,
            "topLogprobs": 3,
            "parallel_function_calling": false
        }))),
        tools: Some(vec![weather_tool()]),
        tool_choice: Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "get_weather".into(),
        }),
        ..Default::default()
    };

    let (body, warnings) = model.get_args(&options).expect("get_args");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    insta::assert_json_snapshot!("chat_request_body", body);
}

/// Golden of the full Responses request body: `max_output_tokens`, nested
/// `reasoning: { effort, summary }`, `text.format` json_schema, `store: false`
/// auto-appending `reasoning.encrypted_content` to `include`,
/// `previous_response_id`, typed `input` items, provider tools mapped by id,
/// and a sanitized function tool.
#[test]
fn responses_request_body_golden() {
    let model = create_xai(XaiProviderSettings {
        base_url: Some("https://api.x.ai/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .responses("grok-4.5");

    let options = LanguageModelV4CallOptions {
        prompt: vec![
            LanguageModelV4Message::system("Be brief."),
            LanguageModelV4Message::user_text("what is xAI?"),
        ],
        max_output_tokens: Some(512),
        temperature: Some(0.2),
        top_p: Some(0.8),
        seed: Some(7),
        response_format: Some(ResponseFormat::Json {
            schema: Some(json!({
                "type": "object",
                "properties": { "summary": { "type": "string" } }
            })),
            name: Some("report".to_string()),
            description: Some("A short report".to_string()),
        }),
        provider_options: Some(xai_provider_options(json!({
            "reasoningEffort": "low",
            "reasoningSummary": "auto",
            "store": false,
            "previousResponseId": "resp_123",
            "topLogprobs": 2
        }))),
        tools: Some(vec![
            weather_tool(),
            LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::from_id(
                "xai.web_search",
                "web_search",
            )),
        ]),
        tool_choice: Some(LanguageModelV4ToolChoice::Auto),
        ..Default::default()
    };

    let (body, warnings, _names) = model.get_args(&options).expect("get_args");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    insta::assert_json_snapshot!("responses_request_body", body);
}
