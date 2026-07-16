//! Cassette replay harness for the xAI **chat** model: the real captured SSE
//! transcripts from `@ai-sdk/xai`'s `__fixtures__` (grok-3-mini reasoning +
//! text, and a complete-in-one-delta tool call) are replayed through the REAL
//! provider + SSE codec over a loopback server. The recorded request is derived
//! from the same `get_args` the provider uses, so `player.verify()` asserts the
//! outbound request body matched — the request-shape guard wiremock lacks.
//!
//! Mirrors the anthropic/openai cassette-replay harness. Expected values are
//! pinned from the upstream TS fixtures so a silent codec drift (block
//! ordering, usage math, dropped events) shows up here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_cassette::Cassette;
use coco_cassette::CassettePlayer;
use coco_cassette::Interaction;
use coco_cassette::RecordedRequest;
use coco_cassette::RecordedResponse;
use futures::StreamExt;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::UnifiedFinishReason;
use vercel_ai_xai::XaiProviderSettings;
use vercel_ai_xai::create_xai;

/// Wrap a one-JSON-per-line fixture (the upstream `.chunks.txt` format) into
/// an SSE body, exactly as the TS test harness does.
fn fixture_to_sse(fixture: &str) -> String {
    let mut body = String::new();
    for line in fixture.lines().filter(|l| !l.trim().is_empty()) {
        body.push_str("data: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn one_shot_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        ..Default::default()
    }
}

/// Run one chat fixture through the real codec via a cassette and return the
/// decoded parts.
async fn replay_chat_fixture(fixture: &str) -> Vec<LanguageModelV4StreamPart> {
    let options = one_shot_options();

    // Derive the recorded request from the same get_args the provider sends,
    // plus the exact streaming fields do_stream adds.
    let probe = create_xai(XaiProviderSettings::api_key(
        Some("https://api.x.ai/v1".to_string()),
        Some("test-key".to_string()),
    ))
    .chat("grok-3-mini");
    let (mut recorded_body, _warnings) = probe.get_args(&options).expect("get_args");
    recorded_body["stream"] = json!(true);
    recorded_body["stream_options"] = json!({ "include_usage": true });

    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".to_string(),
            path: "/chat/completions".to_string(),
            body: recorded_body,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: fixture_to_sse(fixture),
        },
    }]);

    let player = CassettePlayer::start(cassette).await;
    let model = create_xai(XaiProviderSettings::api_key(
        Some(player.base_url()),
        Some("test-key".to_string()),
    ))
    .chat("grok-3-mini");

    let mut stream = model
        .do_stream(&options, None)
        .await
        .expect("do_stream opens");
    let mut parts = Vec::new();
    while let Some(part) = stream.stream.next().await {
        parts.push(part.expect("stream part decodes"));
    }
    player.verify();
    parts
}

/// Run-length-encode the parts' serde `type` tags into a compact, stable
/// shape string (`text-delta x626` instead of 626 lines).
fn rle_shape(parts: &[LanguageModelV4StreamPart]) -> String {
    let mut out: Vec<(String, usize)> = Vec::new();
    for part in parts {
        let tag = serde_json::to_value(part)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
            .unwrap_or_else(|| "<unknown>".to_string());
        match out.last_mut() {
            Some((last, n)) if *last == tag => *n += 1,
            _ => out.push((tag, 1)),
        }
    }
    out.iter()
        .map(|(tag, n)| {
            if *n == 1 {
                tag.clone()
            } else {
                format!("{tag} x{n}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Golden of the DECODED stream shape for the real grok-3-mini reasoning+text
/// transcript, plus value asserts pinned from the fixture: reasoning arrives
/// via `reasoning_content` deltas, text closes the reasoning block, and the
/// final `choices: []` chunk carries top-level usage with *additive*
/// reasoning tokens (completion 1 + reasoning 290 → output total 291).
#[tokio::test]
async fn cassette_replays_chat_text_fixture() {
    let parts = replay_chat_fixture(include_str!("fixtures/xai-chat-text.chunks.txt")).await;

    insta::assert_snapshot!("chat_text_stream_shape", rle_shape(&parts));

    let mut reasoning = String::new();
    let mut text = String::new();
    let mut finish = None;
    for part in &parts {
        match part {
            LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => reasoning.push_str(delta),
            LanguageModelV4StreamPart::TextDelta { delta, .. } => text.push_str(delta),
            LanguageModelV4StreamPart::Finish {
                usage,
                finish_reason,
                ..
            } => finish = Some((usage.clone(), finish_reason.clone())),
            _ => {}
        }
    }
    assert_eq!(reasoning, "First, the user said");
    assert_eq!(text, "Hello");

    let (usage, finish_reason) = finish.expect("finish part");
    assert_eq!(finish_reason.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(finish_reason.raw.as_deref(), Some("stop"));
    // prompt 12 with cached 11 (inclusive), completion 1 + reasoning 290.
    assert_eq!(usage.input_tokens.total(), Some(12));
    assert_eq!(usage.input_tokens.cache_read(), Some(11));
    assert_eq!(usage.output_tokens.total, Some(291));
    assert_eq!(usage.output_tokens.text, Some(1));
    assert_eq!(usage.output_tokens.reasoning, Some(290));
}

/// The real complete-in-one-delta tool-call transcript: the single delta must
/// expand to the `tool-input-start → delta → end → tool-call` quartet after
/// the reasoning block closes, and finish maps `tool_calls → ToolUse`.
#[tokio::test]
async fn cassette_replays_chat_tool_call_fixture() {
    let parts = replay_chat_fixture(include_str!("fixtures/xai-chat-tool-call.chunks.txt")).await;

    insta::assert_snapshot!("chat_tool_call_stream_shape", rle_shape(&parts));

    let mut tool_call = None;
    let mut finish = None;
    for part in &parts {
        match part {
            LanguageModelV4StreamPart::ToolCall(tc) => tool_call = Some(tc.clone()),
            LanguageModelV4StreamPart::Finish {
                usage,
                finish_reason,
                ..
            } => finish = Some((usage.clone(), finish_reason.clone())),
            _ => {}
        }
    }

    let tc = tool_call.expect("tool call decoded");
    assert_eq!(tc.tool_call_id, "call_55117580");
    assert_eq!(tc.tool_name, "weather");
    let input: serde_json::Value = serde_json::from_str(&tc.input).expect("valid args");
    assert_eq!(input, json!({ "location": "San Francisco" }));

    let (usage, finish_reason) = finish.expect("finish part");
    assert_eq!(finish_reason.unified, UnifiedFinishReason::ToolUse);
    // prompt 291 with cached 290 (inclusive), completion 26 + reasoning 196.
    assert_eq!(usage.input_tokens.total(), Some(291));
    assert_eq!(usage.input_tokens.cache_read(), Some(290));
    assert_eq!(usage.output_tokens.total, Some(222));
    assert_eq!(usage.output_tokens.text, Some(26));
    assert_eq!(usage.output_tokens.reasoning, Some(196));
}
