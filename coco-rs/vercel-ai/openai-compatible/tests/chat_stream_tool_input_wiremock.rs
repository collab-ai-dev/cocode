//! Wire-level streaming repair tests for OpenAI-compatible Chat.
//!
//! Same SSE shape as `vercel-ai-openai`; this exists to lock the
//! contract on the OpenAI-compat code path which uses the shared
//! `StreamingToolCallTracker` from `vercel-ai-provider-utils`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_openai_compatible::OpenAICompatibleProviderSettings;
use vercel_ai_openai_compatible::create_openai_compatible;
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
use wiremock::matchers::path;

fn sse_body_with_arguments(arguments: &str) -> String {
    let chunk_open = json!({
        "id": "chatcmpl-compat",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "compat-test",
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": "call_compat",
                    "type": "function",
                    "function": {"name": "Read", "arguments": ""},
                }],
            },
            "finish_reason": null,
        }],
    });
    let chunk_args = json!({
        "id": "chatcmpl-compat",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "compat-test",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {"arguments": arguments},
                }],
            },
            "finish_reason": null,
        }],
    });
    let chunk_finish = json!({
        "id": "chatcmpl-compat",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "compat-test",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
    });
    format!(
        "data: {chunk_open}\n\ndata: {chunk_args}\n\ndata: {chunk_finish}\n\ndata: [DONE]\n\n",
        chunk_open = serde_json::to_string(&chunk_open).unwrap(),
        chunk_args = serde_json::to_string(&chunk_args).unwrap(),
        chunk_finish = serde_json::to_string(&chunk_finish).unwrap(),
    )
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

async fn dispatch_stream(raw_arguments: &str) -> LanguageModelV4ToolCall {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body_with_arguments(raw_arguments), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_openai_compatible(OpenAICompatibleProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        name: Some("compat".into()),
        ..Default::default()
    });
    let model = provider.chat("compat-test");
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
async fn compat_stream_clean_arguments() {
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x"}"#).await;
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_stream_markdown_fence_passes_through() {
    let raw = "```json\n{\"file_path\": \"/r/main.rs\"}\n```";
    let tc = dispatch_stream(raw).await;
    assert_eq!(tc.input, raw);
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_stream_trailing_comma_passes_through() {
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x", "limit": 100,}"#).await;
    assert!(tc.input.contains("100,"));
    assert!(!tc.invalid);
}
