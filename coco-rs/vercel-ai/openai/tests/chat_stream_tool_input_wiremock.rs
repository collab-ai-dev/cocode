//! Wire-level streaming repair tests for OpenAI Chat Completions.
//!
//! Drives `do_stream` against an SSE wiremock fixture and asserts
//! the final `LanguageModelV4StreamPart::ToolCall` emitted by the
//! provider's `StreamingToolCallTracker`. Locks the contract that
//! repair fires when the model's chunked `arguments` accumulation
//! produces malformed output.
//!
//! The fixture sends *one* `data:` line per stream — chunking
//! the arguments across multiple deltas wouldn't add coverage
//! (the tracker concatenates either way) and would make the test
//! fixture much larger.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::create_openai;
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

/// Build an SSE body with one `chat.completion.chunk` carrying the
/// full `arguments` string, followed by the finish chunk and `[DONE]`.
/// Mirrors what `api.openai.com/v1/chat/completions` returns for a
/// streamed tool_use completion.
fn sse_body_with_arguments(arguments: &str) -> String {
    // First chunk: open the tool call (id + name, empty args).
    let chunk_open = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": "call_abc",
                    "type": "function",
                    "function": {"name": "Read", "arguments": ""},
                }],
            },
            "finish_reason": null,
        }],
    });
    // Second chunk: the full args (concatenated into the tracker).
    let chunk_args = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "gpt-test",
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
    // Finish chunk.
    let chunk_finish = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "gpt-test",
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

/// Drive `do_stream` and return the final `LanguageModelV4ToolCall`
/// emitted on the stream. Panics if no tool call arrives — the
/// tests construct fixtures that guarantee one.
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

    let provider = create_openai(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    });
    let model = provider.chat("gpt-test");
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
async fn openai_chat_stream_clean_arguments() {
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x"}"#).await;
    assert_eq!(tc.tool_name, "Read");
    // Stream-level tool call carries `input: String`; assert the
    // stringified form (downstream consumers parse via Layer 1).
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn openai_chat_stream_trailing_comma_passes_string_through() {
    // OpenAI Chat streaming tracker doesn't repair — it accumulates
    // the string and emits it; the engine consumer
    // (`parse_tool_arguments_or_empty`) repairs at the reconstruction
    // seam. This test pins the contract that the string arrives
    // **intact** so the downstream repair has full input.
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x", "limit": 100,}"#).await;
    assert!(
        tc.input.contains("100,"),
        "trailing comma should survive to the engine reconstruction: {}",
        tc.input
    );
    assert!(!tc.invalid);
}

#[tokio::test]
async fn openai_chat_stream_markdown_fence_passes_through() {
    let raw = "```json\n{\"file_path\": \"/r/main.rs\"}\n```";
    let tc = dispatch_stream(raw).await;
    assert_eq!(tc.input, raw);
    assert!(!tc.invalid);
}
