//! Wire-level tests for the xAI chat model: `do_generate` response parsing
//! (text / reasoning_content / tool_calls / citations / usage) and `do_stream`
//! (deltas + top-level usage + complete-in-one-piece tool calls).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::UnifiedFinishReason;
use vercel_ai_xai::XaiProviderSettings;
use vercel_ai_xai::create_xai;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        ..Default::default()
    }
}

#[tokio::test]
async fn do_generate_parses_text_reasoning_tools_citations_and_usage() {
    let server = MockServer::start().await;
    let body = json!({
        "id": "chatcmpl-1",
        "created": 1_700_000_000,
        "model": "grok-4.5",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello!",
                "reasoning_content": "thinking...",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "citations": ["https://example.com/1", "https://example.com/2"],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "total_tokens": 30,
            "completion_tokens_details": {"reasoning_tokens": 5}
        }
    });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let result = model
        .do_generate(&options(), None)
        .await
        .expect("do_generate");

    let mut saw_text = false;
    let mut saw_reasoning = false;
    let mut saw_tool = false;
    let mut sources = 0;
    for part in &result.content {
        match part {
            AssistantContentPart::Text(t) => {
                assert_eq!(t.text, "Hello!");
                saw_text = true;
            }
            AssistantContentPart::Reasoning(r) => {
                assert_eq!(r.text, "thinking...");
                saw_reasoning = true;
            }
            AssistantContentPart::ToolCall(tc) => {
                assert_eq!(tc.tool_name, "get_weather");
                assert_eq!(tc.input, json!({"city": "SF"}));
                saw_tool = true;
            }
            AssistantContentPart::Source(s) => {
                assert!(
                    s.url
                        .as_deref()
                        .unwrap()
                        .starts_with("https://example.com/")
                );
                sources += 1;
            }
            _ => {}
        }
    }
    assert!(saw_text && saw_reasoning && saw_tool);
    assert_eq!(sources, 2);

    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolUse);
    assert_eq!(result.usage.input_tokens.total(), Some(10));
    // Reasoning tokens are additive to the completion total.
    assert_eq!(result.usage.output_tokens.total, Some(25));
    assert_eq!(result.usage.output_tokens.text, Some(20));
    assert_eq!(result.usage.output_tokens.reasoning, Some(5));
}

/// Recover the retryability flag the model attaches via the `APICallError`
/// cause, exactly as the inference retry classifier does.
fn retryable_of(err: &AISdkError) -> Option<bool> {
    err.cause
        .as_deref()
        .and_then(|c| c.downcast_ref::<APICallError>())
        .map(|api| api.is_retryable)
}

#[tokio::test]
async fn do_generate_soft_error_on_200_is_error() {
    // `code` is not the transient-outage sentinel, so the soft error is a
    // non-retryable HTTP-200 error carrying an APICallError cause.
    let server = MockServer::start().await;
    let body = json!({ "code": "boom", "error": "The service is currently unavailable" });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("k".into()),
    ));
    let model = provider.chat("grok-4.5");
    let err = model
        .do_generate(&options(), None)
        .await
        .expect_err("soft error must surface");
    assert_eq!(
        retryable_of(&err),
        Some(false),
        "code=boom is not retryable"
    );
}

#[tokio::test]
async fn do_generate_soft_error_service_unavailable_is_retryable() {
    // The exact `code` sentinel marks a transient outage: the attached
    // APICallError cause must be retryable so the inference classifier retries,
    // mirroring the TS `isRetryable: code === 'The service is currently unavailable'`.
    let server = MockServer::start().await;
    let body = json!({
        "code": "The service is currently unavailable",
        "error": "grok is warming up",
    });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("k".into()),
    ));
    let model = provider.chat("grok-4.5");
    let err = model
        .do_generate(&options(), None)
        .await
        .expect_err("soft error must surface");
    assert_eq!(
        retryable_of(&err),
        Some(true),
        "service-unavailable sentinel must be retryable"
    );
}

#[tokio::test]
async fn do_stream_emits_deltas_sources_and_top_level_usage() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"c1\",\"created\":1700000000,\"model\":\"grok-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"th\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"citations\":[\"https://example.com/a\"]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"completion_tokens_details\":{\"reasoning_tokens\":5}}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut sources = 0;
    let mut finish_usage = None;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::TextDelta { delta, .. } => text.push_str(&delta),
            LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => reasoning.push_str(&delta),
            LanguageModelV4StreamPart::Source(_) => sources += 1,
            LanguageModelV4StreamPart::Finish {
                usage,
                finish_reason: fr,
                ..
            } => {
                finish_usage = Some(usage);
                finish_reason = Some(fr);
            }
            _ => {}
        }
    }

    assert_eq!(text, "Hello");
    assert_eq!(reasoning, "th");
    assert_eq!(sources, 1);
    let usage = finish_usage.expect("finish usage");
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.output_tokens.total, Some(25));
    assert_eq!(usage.output_tokens.text, Some(20));
    assert_eq!(usage.output_tokens.reasoning, Some(5));
    assert_eq!(
        finish_reason.expect("finish reason").unified,
        UnifiedFinishReason::EndTurn
    );
}

#[tokio::test]
async fn do_stream_surfaces_200_json_error_body() {
    // A soft error delivered as HTTP 200 with `content-type: application/json`
    // (not SSE) must surface as an error, not a silently-empty stream.
    let server = MockServer::start().await;
    let body = json!({ "code": "boom", "error": "The service is currently unavailable" });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(body),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let result = model.do_stream(&options(), None).await;
    let err = result.expect_err("200-with-JSON-error must surface as an error");
    // `code` is not the sentinel, so this error is not retryable, and the
    // surfaced message is the verbatim `error` text (no `code:` prefix).
    assert_eq!(retryable_of(&err), Some(false));
    assert!(
        err.message.contains("The service is currently unavailable"),
        "message should carry the verbatim error text, got: {}",
        err.message
    );
}

#[tokio::test]
async fn do_stream_200_json_error_service_unavailable_is_retryable() {
    // Same non-SSE 200 error path, but the `code` sentinel marks it retryable.
    let server = MockServer::start().await;
    let body = json!({
        "code": "The service is currently unavailable",
        "error": "grok is warming up",
    });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(body),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let err = model
        .do_stream(&options(), None)
        .await
        .expect_err("200-with-JSON-error must surface as an error");
    assert_eq!(retryable_of(&err), Some(true));
}

#[tokio::test]
async fn do_stream_emits_complete_tool_call_in_one_delta() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"SF\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut tool_call: Option<LanguageModelV4ToolCall> = None;
    let mut saw_input_start = false;
    let mut saw_input_end = false;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::ToolInputStart { tool_name, .. } => {
                assert_eq!(tool_name, "get_weather");
                saw_input_start = true;
            }
            LanguageModelV4StreamPart::ToolInputEnd { .. } => saw_input_end = true,
            LanguageModelV4StreamPart::ToolCall(tc) => tool_call = Some(tc),
            LanguageModelV4StreamPart::Finish {
                finish_reason: fr, ..
            } => finish_reason = Some(fr),
            _ => {}
        }
    }

    assert!(saw_input_start && saw_input_end);
    let tc = tool_call.expect("a tool call");
    assert_eq!(tc.tool_name, "get_weather");
    assert_eq!(tc.tool_call_id, "call_1");
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).expect("valid json args");
    assert_eq!(parsed, json!({"city": "SF"}));
    assert_eq!(
        finish_reason.expect("finish reason").unified,
        UnifiedFinishReason::ToolUse
    );
}

#[tokio::test]
async fn do_stream_malformed_chunk_emits_error_part_and_other_finish() {
    // A malformed mid-stream chunk surfaces as an `Error` stream part; the
    // finish reason stays `Other` (raw "error"), NOT the `Error` unified variant
    // — matching the TS xAI reference and the openai / openai-compatible /
    // google / anthropic majority. The error part is the real signal.
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"c1\",\"created\":1700000000,\"model\":\"grok-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {not valid json}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ));
    let model = provider.chat("grok-4.5");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut saw_error = false;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::Error { .. } => saw_error = true,
            LanguageModelV4StreamPart::Finish {
                finish_reason: fr, ..
            } => finish_reason = Some(fr),
            _ => {}
        }
    }

    assert!(
        saw_error,
        "malformed chunk must surface an Error stream part"
    );
    let fr = finish_reason.expect("finish reason");
    assert_eq!(
        fr.unified,
        UnifiedFinishReason::Other,
        "unified finish stays Other, not Error"
    );
    assert_eq!(fr.raw.as_deref(), Some("error"), "raw marker preserved");
}
