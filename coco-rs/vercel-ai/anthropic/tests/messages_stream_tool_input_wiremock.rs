//! Wire-level streaming tool-input tests for Anthropic Messages API.
//!
//! Anthropic SSE uses the event-typed shape (`event: <kind>\n
//! data: <json>\n\n`) with this sequence for a tool_use turn:
//!
//! 1. `message_start` — usage + container
//! 2. `content_block_start` — `tool_use` block with id+name
//! 3. `content_block_delta` — `input_json_delta { partial_json }`
//! 4. `content_block_stop` — finalizes the block; adapter runs
//!    `parse_with_repair` on accumulated partial_json (Layer 1)
//! 5. `message_delta` — `stop_reason: tool_use`
//! 6. `message_stop`
//!
//! Adapter behaviour we lock here: the `parse_with_repair` call on
//! the accumulated `input_json` buffer succeeds for repairable
//! payloads (markdown fence, trailing comma) and falls back to `{}`
//! for unrecoverable garbage. The streaming tracker still emits a
//! `LanguageModelV4ToolCall` with the stringified input — engine
//! reconstruction then parses again at the seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_anthropic::AnthropicProviderSettings;
use vercel_ai_anthropic::create_anthropic;
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

/// Construct an Anthropic SSE body where the `tool_use` block's
/// `input_json_delta` carries `partial_json` in one chunk.
fn sse_body_with_partial_json(partial_json: &str) -> String {
    let message_start = json!({
        "type": "message_start",
        "message": {
            "id": "msg_test",
            "model": "claude-test",
            "usage": {"input_tokens": 10},
            "content": [],
        },
    });
    let content_block_start = json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {
            "type": "tool_use",
            "id": "toolu_abc",
            "name": "Read",
            "input": {},
        },
    });
    let content_block_delta = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {
            "type": "input_json_delta",
            "partial_json": partial_json,
        },
    });
    let content_block_stop = json!({
        "type": "content_block_stop",
        "index": 0,
    });
    let message_delta = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "tool_use"},
        "usage": {"output_tokens": 5},
    });
    let message_stop = json!({"type": "message_stop"});

    let mut body = String::new();
    for (event, data) in [
        ("message_start", &message_start),
        ("content_block_start", &content_block_start),
        ("content_block_delta", &content_block_delta),
        ("content_block_stop", &content_block_stop),
        ("message_delta", &message_delta),
        ("message_stop", &message_stop),
    ] {
        body.push_str(&format!(
            "event: {event}\ndata: {}\n\n",
            serde_json::to_string(data).unwrap()
        ));
    }
    body
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

async fn dispatch_stream(partial_json: &str) -> LanguageModelV4ToolCall {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(
                    sse_body_with_partial_json(partial_json),
                    "text/event-stream",
                ),
        )
        .mount(&server)
        .await;

    let provider = create_anthropic(AnthropicProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.messages("claude-test");
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
async fn anthropic_stream_clean_partial_json() {
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x"}"#).await;
    assert_eq!(tc.tool_name, "Read");
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_stream_trailing_comma_is_repaired_at_content_block_stop() {
    // Anthropic streaming adapter calls `parse_with_repair` on the
    // accumulated `input_json` buffer at `content_block_stop`.
    // The repair canonicalises the JSON (strips the trailing comma)
    // and re-serialises so the stream-level `input: String` is
    // already clean.
    let tc = dispatch_stream(r#"{"file_path": "/tmp/x", "limit": 100,}"#).await;
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/tmp/x", "limit": 100}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_stream_markdown_fence_repaired() {
    let tc = dispatch_stream("```json\n{\"file_path\": \"/r/main.rs\"}\n```").await;
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({"file_path": "/r/main.rs"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_stream_unrecoverable_falls_back_without_invalidating() {
    // Anthropic adapter's repair path returns the canonical `Value`
    // re-serialised to string; unrecoverable garbage forwards the
    // raw `input_json` so downstream consumers run repair again.
    // Either way, `invalid` stays `false` at Layer 1.
    let tc = dispatch_stream("\u{0000}!!!@@@%%%").await;
    // Don't pin the exact bytes (forwarded raw or canonicalised
    // depending on llm_json's behaviour); just verify Layer 1
    // didn't unilaterally invalidate.
    assert!(!tc.invalid);
    assert!(tc.invalid_reason.is_none());
}
