//! Wire-level streaming error tests for the OpenAI Responses API.
//!
//! Drives `do_stream` against an SSE fixture whose payload is an
//! in-band `error` event — HTTP 200 followed by a stream-level error,
//! the exact failure shape that previously collapsed to the opaque
//! "Unknown error". Locks the contract that the emitted
//! `LanguageModelV4StreamPart::Error` carries actionable detail (the
//! provider code, or the raw payload) rather than the bare placeholder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::create_openai;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::StreamError;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn one_shot_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart::new("hi"))],
            provider_options: None,
        }],
        ..Default::default()
    }
}

/// Drive `do_stream` against an SSE body and return the first
/// `StreamError` emitted. Panics if the stream finishes without one.
async fn dispatch_error_stream(sse_body: &str) -> StreamError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body.to_string(), "text/event-stream"),
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

    let mut stream_result = model
        .do_stream(&options, None)
        .await
        .expect("do_stream against wiremock should open");

    while let Some(part) = stream_result.stream.next().await {
        if let Ok(LanguageModelV4StreamPart::Error { error }) = part {
            return error;
        }
    }
    panic!("stream finished without an Error part");
}

#[tokio::test]
async fn responses_stream_error_without_message_falls_back_to_raw() {
    // Server-side failures frequently arrive with null message/code.
    // The mapping must not collapse to the opaque "Unknown error" — the
    // raw payload is the only signal we have, so it must reach the user.
    let body = "data: {\"type\":\"error\"}\n\ndata: [DONE]\n\n";
    let err = dispatch_error_stream(body).await;
    assert_ne!(err.message, "Unknown error");
    assert!(
        err.message.contains("OpenAI responses error"),
        "expected raw-fallback message, got: {}",
        err.message
    );
}

#[tokio::test]
async fn responses_stream_error_with_code_surfaces_code() {
    let body = "data: {\"type\":\"error\",\"code\":\"server_error\"}\n\ndata: [DONE]\n\n";
    let err = dispatch_error_stream(body).await;
    assert_eq!(err.code.as_deref(), Some("server_error"));
    assert!(
        err.message.contains("server_error"),
        "code should appear in the surfaced message: {}",
        err.message
    );
}

#[tokio::test]
async fn responses_stream_error_with_message_passes_through() {
    let body = "data: {\"type\":\"error\",\"message\":\"rate limit reached\",\"code\":\"rate_limit\"}\n\ndata: [DONE]\n\n";
    let err = dispatch_error_stream(body).await;
    assert_eq!(err.message, "rate limit reached");
    assert_eq!(err.code.as_deref(), Some("rate_limit"));
}

/// Collect every stream part for the assertions that need to inspect the
/// terminal `Finish` (not just the first `Error`).
async fn dispatch_all_parts(sse_body: &str) -> Vec<LanguageModelV4StreamPart> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body.to_string(), "text/event-stream"),
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

    let mut stream_result = model
        .do_stream(&options, None)
        .await
        .expect("do_stream against wiremock should open");

    let mut parts = Vec::new();
    while let Some(part) = stream_result.stream.next().await {
        parts.push(part.expect("stream part should decode"));
    }
    parts
}

#[tokio::test]
async fn responses_failed_context_length_routes_to_context_window() {
    // A mid-stream context-window overflow must surface as a typed
    // `ContextWindowExceeded` finish (driving reactive compaction) and NOT
    // as an `Error` part — emitting an error would fail the turn instead of
    // recovering it.
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"too long\"}}}\n\ndata: [DONE]\n\n";
    let parts = dispatch_all_parts(body).await;

    assert!(
        !parts
            .iter()
            .any(|p| matches!(p, LanguageModelV4StreamPart::Error { .. })),
        "context_length_exceeded must not emit an Error part"
    );
    let finish = parts
        .iter()
        .find_map(|p| match p {
            LanguageModelV4StreamPart::Finish { finish_reason, .. } => Some(finish_reason),
            _ => None,
        })
        .expect("stream should end with a Finish");
    assert_eq!(
        finish.unified,
        vercel_ai_provider::UnifiedFinishReason::ContextWindowExceeded
    );
}

#[tokio::test]
async fn responses_failed_quota_emits_fatal_error() {
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"insufficient_quota\",\"message\":\"You exceeded your quota\"}}}\n\ndata: [DONE]\n\n";
    let parts = dispatch_all_parts(body).await;

    let err = parts
        .iter()
        .find_map(|p| match p {
            LanguageModelV4StreamPart::Error { error } => Some(error),
            _ => None,
        })
        .expect("quota failure should emit an Error part");
    assert_eq!(err.code.as_deref(), Some("insufficient_quota"));
    assert!(!err.is_retryable, "quota exhaustion is fatal");
    assert_eq!(err.message, "You exceeded your quota");
}

fn reasoning_end_encrypted(part: &LanguageModelV4StreamPart) -> Option<String> {
    match part {
        LanguageModelV4StreamPart::ReasoningEnd {
            provider_metadata: Some(meta),
            ..
        } => meta
            .0
            .get("openai")
            .and_then(|o| o.get("encryptedContent"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

#[tokio::test]
async fn responses_stream_reasoning_captures_encrypted_content() {
    // The encrypted reasoning blob arrives ONLY on `output_item.done`, after
    // the summary text streamed. It must land on the ReasoningEnd that closes
    // the same segment so the chain-of-thought round-trips (store=false).
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"delta\":\"Thinking\"}\n\n",
        "data: {\"type\":\"response.reasoning_summary_text.done\",\"item_id\":\"rs_1\",\"text\":\"Thinking\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking\"}],\"encrypted_content\":\"ENC_BLOB\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let parts = dispatch_all_parts(body).await;

    // Exactly one ReasoningEnd, and it carries the blob.
    let ends: Vec<_> = parts
        .iter()
        .filter(|p| matches!(p, LanguageModelV4StreamPart::ReasoningEnd { .. }))
        .collect();
    assert_eq!(ends.len(), 1, "exactly one ReasoningEnd");
    assert_eq!(
        reasoning_end_encrypted(ends[0]).as_deref(),
        Some("ENC_BLOB"),
        "ReasoningEnd must carry openai.encryptedContent"
    );
    // The summary delta still surfaced.
    assert!(parts.iter().any(|p| matches!(
        p,
        LanguageModelV4StreamPart::ReasoningDelta { delta, .. } if delta == "Thinking"
    )));
}

#[tokio::test]
async fn responses_stream_encrypted_only_reasoning_round_trips() {
    // store=false reasoning with NO summary text: the segment is opened and
    // closed purely from `output_item.done` so the blob still round-trips.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_2\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_2\",\"summary\":[],\"encrypted_content\":\"ENC2\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let parts = dispatch_all_parts(body).await;

    assert!(
        parts.iter().any(
            |p| matches!(p, LanguageModelV4StreamPart::ReasoningStart { id, .. } if id == "rs_2")
        ),
        "an encrypted-only reasoning item still opens a segment"
    );
    let end = parts
        .iter()
        .find(|p| matches!(p, LanguageModelV4StreamPart::ReasoningEnd { .. }))
        .expect("ReasoningEnd emitted");
    assert_eq!(reasoning_end_encrypted(end).as_deref(), Some("ENC2"));
}

#[tokio::test]
async fn responses_stream_raw_reasoning_channel_marked_text() {
    // The raw `reasoning_text.*` channel surfaces as a distinct segment
    // (`::content` id) marked reasoningType=text so it renders live but is
    // stripped on sendback — never colliding with the summary channel.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_3\"}}\n\n",
        "data: {\"type\":\"response.reasoning_text.delta\",\"item_id\":\"rs_3\",\"content_index\":0,\"delta\":\"raw\"}\n\n",
        "data: {\"type\":\"response.reasoning_text.done\",\"item_id\":\"rs_3\",\"content_index\":0,\"text\":\"raw\"}\n\n",
        "data: [DONE]\n\n",
    );
    let parts = dispatch_all_parts(body).await;

    let start_meta = parts
        .iter()
        .find_map(|p| match p {
            LanguageModelV4StreamPart::ReasoningStart {
                id,
                provider_metadata,
            } if id == "rs_3::content" => Some(provider_metadata),
            _ => None,
        })
        .expect("raw reasoning ReasoningStart on ::content id");
    let rtype = start_meta
        .as_ref()
        .and_then(|m| m.0.get("openai"))
        .and_then(|o| o.get("reasoningType"))
        .and_then(|v| v.as_str());
    assert_eq!(
        rtype,
        Some("text"),
        "raw reasoning marked reasoningType=text"
    );
    assert!(parts.iter().any(|p| matches!(
        p,
        LanguageModelV4StreamPart::ReasoningDelta { id, delta, .. } if id == "rs_3::content" && delta == "raw"
    )));
    assert!(parts.iter().any(|p| matches!(
        p,
        LanguageModelV4StreamPart::ReasoningEnd { id, .. } if id == "rs_3::content"
    )));
}

#[tokio::test]
async fn responses_stream_function_call_round_trips_by_call_id() {
    // Streamed tool calls must correlate by the wire `call_id` (`call_…`),
    // NOT the item id (`fc_…`) — otherwise the `function_call_output` won't
    // match and OpenAI 400s. The item id rides provider_metadata.openai.itemId.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_abc\",\"name\":\"Read\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"file_path\\\":\\\"/x\\\"}\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"arguments\":\"{\\\"file_path\\\":\\\"/x\\\"}\"}\n\n",
        "data: [DONE]\n\n",
    );
    let parts = dispatch_all_parts(body).await;

    // Every emitted tool-input id is the call_id, not the item id.
    for part in &parts {
        match part {
            LanguageModelV4StreamPart::ToolInputStart { id, .. }
            | LanguageModelV4StreamPart::ToolInputDelta { id, .. }
            | LanguageModelV4StreamPart::ToolInputEnd { id, .. } => {
                assert_eq!(id, "call_abc", "tool-input ids must be the call_id");
            }
            _ => {}
        }
    }
    let tc = parts
        .iter()
        .find_map(|p| match p {
            LanguageModelV4StreamPart::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .expect("a ToolCall should be emitted");
    assert_eq!(tc.tool_call_id, "call_abc");
    let item_id = tc
        .provider_metadata
        .as_ref()
        .and_then(|m| m.0.get("openai"))
        .and_then(|o| o.get("itemId"))
        .and_then(|v| v.as_str());
    assert_eq!(item_id, Some("fc_1"), "item id rides openai.itemId");
}

#[tokio::test]
async fn responses_stream_function_call_materialized_from_output_item_done() {
    // A call closed only by `output_item.done` (no arguments.done delta) must
    // still materialize exactly one ToolCall via the fallback arm.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_xyz\",\"name\":\"Noop\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_xyz\",\"name\":\"Noop\",\"arguments\":\"{}\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let parts = dispatch_all_parts(body).await;

    let calls: Vec<_> = parts
        .iter()
        .filter_map(|p| match p {
            LanguageModelV4StreamPart::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1, "exactly one ToolCall, no double-emit");
    assert_eq!(calls[0].tool_call_id, "call_xyz");
}

#[tokio::test]
async fn responses_incomplete_max_tokens_finishes_max_tokens() {
    // `response.incomplete` carries its reason in `incomplete_details.reason`
    // (the literal `status` is just "incomplete"); it must map to MaxTokens so
    // the engine's output-budget escalation fires.
    let body = "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\ndata: [DONE]\n\n";
    let parts = dispatch_all_parts(body).await;

    let finish = parts
        .iter()
        .find_map(|p| match p {
            LanguageModelV4StreamPart::Finish { finish_reason, .. } => Some(finish_reason),
            _ => None,
        })
        .expect("stream should end with a Finish");
    assert_eq!(
        finish.unified,
        vercel_ai_provider::UnifiedFinishReason::MaxTokens
    );
}
