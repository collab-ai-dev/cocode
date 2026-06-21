//! Wire-level streaming tool-input tests for Google Gemini.
//!
//! Gemini's streaming wire shape is **fundamentally different from
//! the cumulative-arguments tracker world**: each SSE `data:` chunk
//! contains a *complete* `GoogleGenerateContentResponse` JSON
//! (alt=sse), and the `function_call.args` field arrives as a
//! structured `Value` in one shot (not a stringified delta).
//!
//! Adapter behaviour we pin:
//! - Object `args` round-trips verbatim (no string parse step)
//! - `Value::String` anomaly passes through to schema validation unchanged
//! - Stream `LanguageModelV4ToolCall.input` carries the JSON-
//!   stringified form of the structured value

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_google::GoogleGenerativeAIProviderSettings;
use vercel_ai_google::create_google_generative_ai;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

fn sse_body_with_args(args: serde_json::Value) -> String {
    let chunk = json!({
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
    });
    format!("data: {}\n\n", serde_json::to_string(&chunk).unwrap())
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

async fn dispatch_stream(args: serde_json::Value) -> LanguageModelV4ToolCall {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(".*:streamGenerateContent$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body_with_args(args), "text/event-stream"),
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

    let mut stream_result = model
        .do_stream(&options, None)
        .await
        .expect("do_stream against wiremock should open");

    while let Some(part) = stream_result.stream.next().await {
        match part {
            Ok(LanguageModelV4StreamPart::ToolCall(tc)) => return tc,
            Ok(_) => continue,
            Err(e) => panic!("stream error: {e}"),
        }
    }
    panic!("stream finished without a ToolCall part");
}

#[tokio::test]
async fn google_stream_object_args_round_trips() {
    let tc = dispatch_stream(json!({"file_path": "/tmp/x"})).await;
    assert_eq!(tc.tool_name, "Read");
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn google_stream_complex_args_round_trip() {
    let tc = dispatch_stream(json!({
        "file_path": "/repo/src/main.rs",
        "limit": 100,
        "offset": 50,
    }))
    .await;
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(
        parsed,
        json!({
            "file_path": "/repo/src/main.rs",
            "limit": 100,
            "offset": 50,
        })
    );
    assert!(!tc.invalid);
}

#[tokio::test]
async fn google_stream_value_string_passes_through() {
    let nested = "{\"file_path\":\"/tmp/recovered\"}";
    let tc = dispatch_stream(json!(nested)).await;
    // String args round-trip as JSON-stringified string (note the
    // outer quoting from `serde_json::to_string`).
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!(nested));
    assert!(!tc.invalid);
}
