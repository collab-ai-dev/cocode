//! Replay a cassette through the REAL OpenAI Chat provider + SSE codec.
//!
//! The high-fidelity counterpart to `chat_stream_tool_input_wiremock.rs`: the
//! recorded request is derived from the same `get_args` the provider sends
//! (plus the `stream` / `stream_options` keys `do_stream` layers on), and
//! replay runs over a real loopback server. So request serialization, SSE
//! framing, the streaming tool-call tracker, and stream accumulation all
//! execute against recorded bytes — no network, no key. `player.verify()` then
//! asserts the interaction was consumed AND the outbound request body matched
//! what we recorded — the request-shape guard the method+path-only wiremock
//! tests lack.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_cassette::Cassette;
use coco_cassette::CassettePlayer;
use coco_cassette::Interaction;
use coco_cassette::RecordedRequest;
use coco_cassette::RecordedResponse;
use futures::StreamExt;
use serde_json::json;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::create_openai;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;

/// An OpenAI Chat Completions streamed tool_use transcript (one `Read` call),
/// stored verbatim as the recorded response body.
fn tool_use_sse() -> String {
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
                    "function": {"arguments": "{\"file_path\": \"/tmp/x\"}"},
                }],
            },
            "finish_reason": null,
        }],
    });
    let chunk_finish = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": "gpt-test",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
    });
    format!(
        "data: {open}\n\ndata: {args}\n\ndata: {finish}\n\ndata: [DONE]\n\n",
        open = serde_json::to_string(&chunk_open).unwrap(),
        args = serde_json::to_string(&chunk_args).unwrap(),
        finish = serde_json::to_string(&chunk_finish).unwrap(),
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

#[tokio::test]
async fn cassette_replays_openai_chat_tool_use_through_real_codec() {
    let options = one_shot_options();

    // Derive the recorded request from the same `get_args` the provider uses,
    // then layer on the `stream` / `stream_options` keys `do_stream` adds — so
    // the recorded body exactly matches the streamed request.
    let probe = create_openai(OpenAIProviderSettings {
        base_url: Some("https://api.openai.com/v1".to_string()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    })
    .chat("gpt-test");
    let (mut recorded_body, _warnings) = probe.get_args(&options).expect("get_args");
    recorded_body["stream"] = serde_json::Value::Bool(true);
    recorded_body["stream_options"] = json!({"include_usage": true});

    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".to_string(),
            path: "/chat/completions".to_string(),
            body: recorded_body,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: tool_use_sse(),
        },
    }]);

    // Replay: point a real provider at the cassette server and stream.
    let player = CassettePlayer::start(cassette).await;
    let model = create_openai(OpenAIProviderSettings {
        base_url: Some(player.base_url()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    })
    .chat("gpt-test");

    let mut stream = model
        .do_stream(&options, None)
        .await
        .expect("do_stream opens");
    let mut tool_call = None;
    while let Some(part) = stream.stream.next().await {
        if let Ok(LanguageModelV4StreamPart::ToolCall(tc)) = part {
            tool_call = Some(tc);
        }
    }

    let tc = tool_call.expect("a ToolCall part was decoded from the replayed SSE");
    assert_eq!(tc.tool_name, "Read");
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).unwrap();
    assert_eq!(parsed, json!({ "file_path": "/tmp/x" }));
    assert!(!tc.invalid);

    // Consumed exactly once AND the outbound request body matched the recording.
    player.verify();
}
