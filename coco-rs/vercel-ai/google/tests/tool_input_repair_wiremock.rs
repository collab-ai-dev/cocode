//! Wire-level tool-input handling tests for Google Gemini
//! (non-streaming).
//!
//! Like Anthropic, Gemini's wire `function_call.args` is a structured
//! JSON `Value`, not a stringified `arguments` field. There is no
//! wire-parsing string repair to exercise; the tests pin that:
//! - Object inputs round-trip verbatim
//! - Empty / Null / odd shapes pass through unchanged (schema validation owns
//!   classification)
//! - `Value::String` anomalies (extremely rare from Gemini but
//!   theoretically possible) reach schema validation intact

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vercel_ai_google::GoogleGenerativeAIProviderSettings;
use vercel_ai_google::create_google_generative_ai;
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

fn generate_content_response_with_args(args: serde_json::Value) -> serde_json::Value {
    json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "functionCall": {
                        "name": "Read",
                        "args": args,
                    },
                }],
                "role": "model",
            },
            "finishReason": "STOP",
        }],
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "totalTokenCount": 15,
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

async fn dispatch(args: serde_json::Value) -> vercel_ai_provider::ToolCallPart {
    let server = MockServer::start().await;
    // Gemini path: `/v1beta/models/{model}:generateContent`.
    // wiremock's `path` matcher does exact match; use partial via
    // `path_regex` so we don't have to spell out the full URL.
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(".*:generateContent$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(generate_content_response_with_args(args)),
        )
        .mount(&server)
        .await;

    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        base_url: Some(format!("{}/v1beta", server.uri())),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .language_model_instance("gemini-test")
        .expect("model instance should build");
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
async fn google_nonstream_object_args_round_trips() {
    let tc = dispatch(json!({"file_path": "/tmp/x", "limit": 100})).await;
    assert_eq!(tc.tool_name, "Read");
    assert_eq!(tc.input, json!({"file_path": "/tmp/x", "limit": 100}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn google_nonstream_empty_object_preserved() {
    let tc = dispatch(json!({})).await;
    assert_eq!(tc.input, json!({}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn google_nonstream_value_string_passes_through() {
    // Cross-protocol anomaly: model nests stringified JSON.
    let nested = "{\"file_path\":\"/tmp/recovered\"}";
    let tc = dispatch(json!(nested)).await;
    assert_eq!(tc.input, json!(nested));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn google_nonstream_null_input_preserved() {
    let tc = dispatch(serde_json::Value::Null).await;
    assert!(matches!(tc.input, serde_json::Value::Null));
    assert!(!tc.invalid);
}
