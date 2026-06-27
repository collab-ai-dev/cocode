//! Wire-level repair tests for OpenAI Responses API.
//!
//! Same pattern as `chat_tool_input_repair_wiremock.rs` but routed
//! through `/responses` and the Responses API wire shape
//! (`output: [{type: "function_call", call_id, name, arguments}]`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::create_openai;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn responses_body_with_arguments(arguments: &str) -> serde_json::Value {
    json!({
        "id": "resp_test",
        "object": "response",
        "created_at": 1_700_000_000,
        "model": "gpt-test",
        "status": "completed",
        "output": [{
            "id": "fc_1",
            "type": "function_call",
            "call_id": "call_abc",
            "name": "Read",
            "arguments": arguments,
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 15,
        },
    })
}

fn one_shot_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart::new("read /tmp/x"))],
            provider_options: None,
        }],
        ..Default::default()
    }
}

async fn dispatch(raw_arguments: &str) -> vercel_ai_provider::ToolCallPart {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(responses_body_with_arguments(raw_arguments)),
        )
        .mount(&server)
        .await;

    let provider = create_openai(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    });
    let model = provider.responses("gpt-test");
    let options = one_shot_options();

    let result = model
        .do_generate(&options, None)
        .await
        .expect("do_generate against wiremock should succeed");

    result
        .content
        .into_iter()
        .find_map(|p| match p {
            AssistantContentPart::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .expect("response contained a tool call by construction")
}

#[tokio::test]
async fn responses_nonstream_clean() {
    let tc = dispatch(r#"{"file_path": "/tmp/x"}"#).await;
    assert_eq!(tc.tool_name, "Read");
    assert_eq!(tc.input, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn responses_nonstream_trailing_comma_repaired() {
    let tc = dispatch(r#"{"file_path": "/tmp/x", "limit": 100,}"#).await;
    assert_eq!(tc.input, json!({"file_path": "/tmp/x", "limit": 100}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn responses_nonstream_markdown_fence_repaired() {
    let tc = dispatch("```json\n{\"file_path\": \"/r/main.rs\"}\n```").await;
    assert_eq!(tc.input, json!({"file_path": "/r/main.rs"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn responses_nonstream_empty_arguments_becomes_empty_object() {
    let tc = dispatch("").await;
    assert_eq!(tc.input, json!({}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn responses_nonstream_single_quotes_repaired() {
    let tc = dispatch(r#"{'file_path': '/tmp/sq'}"#).await;
    assert_eq!(tc.input, json!({"file_path": "/tmp/sq"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn responses_nonstream_unrecoverable_layer_1_does_not_invalidate() {
    let tc = dispatch("\u{0000}!!!@@@%%%").await;
    // Raw bytes preserved (Value::String) or salvaged (any non-empty
    // Value) — wire parsing never silently drops to `{}`.
    assert_ne!(tc.input, serde_json::json!({}));
    assert!(!tc.invalid);
}
