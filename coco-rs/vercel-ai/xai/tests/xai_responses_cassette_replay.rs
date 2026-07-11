//! Cassette replay harness for the xAI **Responses** model: the real captured
//! SSE transcripts from `@ai-sdk/xai`'s `responses/__fixtures__` are replayed
//! through the REAL provider + SSE codec over a loopback server.
//!
//! Expected values are pinned from the upstream TS snapshot
//! (`xai-responses-language-model.test.ts.snap`), so this asserts parity with
//! the TS decoder on real wire data: reasoning-summary block lifecycle, text
//! deltas, url_citation sources, provider-executed web_search tool calls, and
//! the inclusive-reasoning usage math.

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
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4Tool;
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
    body
}

/// Run one Responses fixture through the real codec via a cassette.
async fn replay_responses_fixture(
    fixture: &str,
    options: LanguageModelV4CallOptions,
) -> Vec<LanguageModelV4StreamPart> {
    let probe = create_xai(XaiProviderSettings {
        base_url: Some("https://api.x.ai/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .responses("grok-code-fast-1");
    let (mut recorded_body, _warnings, _names) = probe.get_args(&options).expect("get_args");
    recorded_body["stream"] = json!(true);

    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".to_string(),
            path: "/responses".to_string(),
            body: recorded_body,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: fixture_to_sse(fixture),
        },
    }]);

    let player = CassettePlayer::start(cassette).await;
    let model = create_xai(XaiProviderSettings {
        base_url: Some(player.base_url()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .responses("grok-code-fast-1");

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

/// Real grok-code-fast-1 transcript: reasoning summary (59 deltas) then text
/// (626 deltas). Expected shape and usage pinned from the TS snapshot
/// (`should stream text deltas 1`): input 216 (cacheRead 192, noCache 24),
/// output 863 with *inclusive* reasoning 237 → text 626.
#[tokio::test]
async fn cassette_replays_responses_text_reasoning_fixture() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text(
            "What is notable about Sonoran food?",
        )],
        ..Default::default()
    };
    let parts = replay_responses_fixture(
        include_str!("fixtures/xai-responses-text.chunks.txt"),
        options,
    )
    .await;

    insta::assert_snapshot!("responses_text_stream_shape", rle_shape(&parts));

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
    assert!(
        reasoning.starts_with("First, the question is"),
        "reasoning summary text should accumulate from deltas, got: {}",
        &reasoning[..reasoning.len().min(60)]
    );
    assert!(
        text.starts_with("### Overview of Sonoran Cuisine"),
        "text should accumulate from deltas"
    );

    let (usage, finish_reason) = finish.expect("finish part");
    assert_eq!(finish_reason.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(finish_reason.raw.as_deref(), Some("completed"));
    assert_eq!(usage.input_tokens.total(), Some(216));
    assert_eq!(usage.input_tokens.cache_read(), Some(192));
    assert_eq!(usage.input_tokens.no_cache(), Some(24));
    // Responses usage counts reasoning *inclusive* in output_tokens.
    assert_eq!(usage.output_tokens.total, Some(863));
    assert_eq!(usage.output_tokens.text, Some(626));
    assert_eq!(usage.output_tokens.reasoning, Some(237));
}

/// Real grok-4-fast-reasoning Live-Search transcript: a provider-executed
/// `web_search` call (quartet + empty tool-result), url_citation sources from
/// both `annotation.added` and `output_text.done` (10 total, pinned from the
/// TS snapshot), 259 text deltas, and usage input 1875 / output 695.
#[tokio::test]
async fn cassette_replays_responses_web_search_fixture() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("what is xAI?")],
        tools: Some(vec![LanguageModelV4Tool::Provider(
            LanguageModelV4ProviderTool::from_id("xai.web_search", "web_search"),
        )]),
        ..Default::default()
    };
    let parts = replay_responses_fixture(
        include_str!("fixtures/xai-responses-web-search.chunks.txt"),
        options,
    )
    .await;

    insta::assert_snapshot!("responses_web_search_stream_shape", rle_shape(&parts));

    let mut tool_call = None;
    let mut tool_result = None;
    let mut sources = 0;
    let mut text_deltas = 0;
    let mut finish = None;
    for part in &parts {
        match part {
            LanguageModelV4StreamPart::ToolCall(tc) => tool_call = Some(tc.clone()),
            LanguageModelV4StreamPart::ToolResult(tr) => tool_result = Some(tr.clone()),
            LanguageModelV4StreamPart::Source(_) => sources += 1,
            LanguageModelV4StreamPart::TextDelta { .. } => text_deltas += 1,
            LanguageModelV4StreamPart::Finish {
                usage,
                finish_reason,
                ..
            } => finish = Some((usage.clone(), finish_reason.clone())),
            _ => {}
        }
    }

    let tc = tool_call.expect("web_search tool call decoded");
    assert_eq!(tc.tool_name, "web_search");
    assert_eq!(tc.provider_executed, Some(true));
    assert_eq!(tc.input, "{\"query\":\"what is xAI\",\"num_results\":5}");

    let tr = tool_result.expect("web_search tool result decoded");
    assert_eq!(tr.tool_name, "web_search");

    // Pinned from the TS snapshot: 5 annotation.added + 5 output_text.done.
    assert_eq!(sources, 10, "url_citation sources");
    assert_eq!(text_deltas, 259);

    let (usage, finish_reason) = finish.expect("finish part");
    assert_eq!(finish_reason.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(usage.input_tokens.total(), Some(1875));
    assert_eq!(usage.input_tokens.cache_read(), Some(1578));
    assert_eq!(usage.output_tokens.total, Some(695));
    assert_eq!(usage.output_tokens.text, Some(298));
    assert_eq!(usage.output_tokens.reasoning, Some(397));
}
