//! Seam-A demo: replay a cassette through the REAL Anthropic provider + SSE
//! codec.
//!
//! This is the high-fidelity counterpart to the hand-written `*_wiremock.rs`
//! tests. The cassette's recorded request is derived from the same `get_args`
//! the provider uses, and replay runs over a real loopback server, so the
//! genuine path — request serialization, SSE framing, `parse_with_repair`,
//! stream accumulation — executes against recorded bytes with no network and
//! no key. `player.verify()` then asserts the interaction was consumed and the
//! request body matched (the guard the method+path-only wiremock tests lack).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_cassette::Cassette;
use coco_cassette::CassettePlayer;
use coco_cassette::Interaction;
use coco_cassette::RecordedRequest;
use coco_cassette::RecordedResponse;
use futures::StreamExt;
use serde_json::json;
use vercel_ai_anthropic::AnthropicProviderSettings;
use vercel_ai_anthropic::create_anthropic;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;

/// An Anthropic tool_use SSE transcript (one `Read` call), stored verbatim as
/// the recorded response body.
fn tool_use_sse() -> String {
    let events: [(&str, serde_json::Value); 6] = [
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_test","model":"claude-test","usage":{"input_tokens":10},"content":[]}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_abc","name":"Read","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\": \"/tmp/x\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ];
    let mut body = String::new();
    for (event, data) in events {
        body.push_str(&format!(
            "event: {event}\ndata: {}\n\n",
            serde_json::to_string(&data).unwrap()
        ));
    }
    body
}

fn one_shot_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions::new(vec![LanguageModelV4Message::user_text("read /tmp/x")])
}

/// Golden of the DECODED response stream (not just the request body): replay the
/// canonical tool_use SSE through the real Anthropic codec and snapshot the
/// ordered sequence of decoded stream-part *types*. A silent change in how the
/// adapter frames the SSE (block start/delta/stop ordering, a missing/extra
/// event, a renamed part) shows up here — the response-side counterpart to
/// `messages_request_body_golden`. We snapshot the type tags rather than the
/// full `Debug` because the latter embeds the provider's raw usage blob, whose
/// JSON object-key order is non-deterministic under `serde_json/preserve_order`
/// feature unification. The decoded *values* (tool name, input, finish reason)
/// are asserted by `cassette_replays_tool_use_through_real_codec`.
#[tokio::test]
async fn golden_tool_use_sse_decoded_stream_shape() {
    let options = one_shot_options();
    let probe = create_anthropic(AnthropicProviderSettings {
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .messages("claude-test");
    let (recorded_body, _headers, _warnings) = probe.get_args(&options, true).expect("get_args");

    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".to_string(),
            path: "/messages".to_string(),
            body: recorded_body,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: tool_use_sse(),
        },
    }]);

    let player = CassettePlayer::start(cassette).await;
    let model = create_anthropic(AnthropicProviderSettings {
        base_url: Some(player.base_url()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .messages("claude-test");

    let mut stream = model
        .do_stream(&options, None)
        .await
        .expect("do_stream opens");
    let mut event_types: Vec<String> = Vec::new();
    while let Some(part) = stream.stream.next().await {
        let part = part.expect("stream part decodes");
        // The `type` tag is the variant's stable serde discriminant — robust to
        // provider-blob field ordering, unlike the full `Debug`.
        let tag = serde_json::to_value(&part)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
            .unwrap_or_else(|| "<unknown>".to_string());
        event_types.push(tag);
    }

    player.verify();
    insta::assert_snapshot!("tool_use_event_sequence", event_types.join("\n"));
}

#[tokio::test]
async fn cassette_replays_tool_use_through_real_codec() {
    let options = one_shot_options();

    // Derive the recorded request from the same get_args the provider sends, so
    // the body match is exact and self-maintaining rather than hand-pinned.
    let probe = create_anthropic(AnthropicProviderSettings {
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .messages("claude-test");
    let (recorded_body, _headers, _warnings) = probe.get_args(&options, true).expect("get_args");

    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".to_string(),
            path: "/messages".to_string(),
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
    let model = create_anthropic(AnthropicProviderSettings {
        base_url: Some(player.base_url()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .messages("claude-test");

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

    // The interaction was consumed exactly once and the outbound request body
    // matched what we recorded — the request-shape guard wiremock lacks.
    player.verify();
}
