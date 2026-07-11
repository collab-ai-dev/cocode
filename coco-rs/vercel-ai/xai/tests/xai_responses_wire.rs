//! Wire-level tests for the xAI Responses model: `do_generate` output-item
//! parsing (text / reasoning / function-call / url-citation / usage) and
//! `do_stream` (text + reasoning-summary + function-call-args deltas +
//! finish/usage), plus the HTTP-200 soft-error paths.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
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

fn sse(chunks: &[Value]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str(&format!("data: {c}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

fn retryable_of(err: &AISdkError) -> Option<bool> {
    err.cause
        .as_deref()
        .and_then(|c| c.downcast_ref::<APICallError>())
        .map(|api| api.is_retryable)
}

#[tokio::test]
async fn do_generate_parses_text_reasoning_toolcall_source_and_usage() {
    let server = MockServer::start().await;
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 1_700_000_000,
        "status": "completed",
        "model": "grok-4.5",
        "output": [
            {
                "type": "reasoning",
                "id": "rs_1",
                "status": "completed",
                "summary": [{ "type": "summary_text", "text": "thinking..." }]
            },
            {
                "type": "message",
                "id": "msg_1",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Hello!",
                    "annotations": [
                        { "type": "url_citation", "url": "https://example.com/1", "title": "Ex" }
                    ]
                }]
            },
            {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call_1",
                "name": "get_weather",
                "arguments": "{\"city\":\"SF\"}"
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 20,
            "output_tokens_details": { "reasoning_tokens": 5 }
        }
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
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
                let meta = r.provider_metadata.as_ref().expect("reasoning metadata");
                assert_eq!(meta.0["xai"]["itemId"], "rs_1");
                saw_reasoning = true;
            }
            AssistantContentPart::ToolCall(tc) => {
                assert_eq!(tc.tool_name, "get_weather");
                assert_eq!(tc.input, json!({ "city": "SF" }));
                saw_tool = true;
            }
            AssistantContentPart::Source(s) => {
                assert_eq!(s.url.as_deref(), Some("https://example.com/1"));
                sources += 1;
            }
            _ => {}
        }
    }
    assert!(saw_text && saw_reasoning && saw_tool);
    assert_eq!(sources, 1);

    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolUse);
    assert_eq!(result.usage.input_tokens.total(), Some(10));
    assert_eq!(result.usage.output_tokens.total, Some(20));
    assert_eq!(result.usage.output_tokens.text, Some(15));
    assert_eq!(result.usage.output_tokens.reasoning, Some(5));
}

#[tokio::test]
async fn do_generate_soft_error_on_200_is_not_retryable() {
    let server = MockServer::start().await;
    let body = json!({ "code": "boom", "error": "The service is currently unavailable" });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("k".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let err = model
        .do_generate(&options(), None)
        .await
        .expect_err("soft error must surface");
    assert_eq!(retryable_of(&err), Some(false));
}

#[tokio::test]
async fn do_generate_soft_error_service_unavailable_is_retryable() {
    let server = MockServer::start().await;
    let body = json!({
        "code": "The service is currently unavailable",
        "error": "grok is warming up",
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("k".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let err = model
        .do_generate(&options(), None)
        .await
        .expect_err("soft error must surface");
    assert_eq!(retryable_of(&err), Some(true));
}

#[tokio::test]
async fn do_stream_text_reasoning_toolcall_and_usage() {
    let server = MockServer::start().await;
    let chunks = vec![
        json!({ "type": "response.created", "response": { "id": "resp_1", "model": "grok-4.5", "created_at": 1_700_000_000 } }),
        json!({ "type": "response.reasoning_summary_part.added", "item_id": "rs_1", "output_index": 0, "summary_index": 0, "part": { "type": "summary_text", "text": "" } }),
        json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "th" }),
        json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "ink" }),
        json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "reasoning", "id": "rs_1", "status": "completed", "summary": [{ "type": "summary_text", "text": "think" }] } }),
        json!({ "type": "response.output_item.added", "output_index": 1, "item": { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "get_weather", "arguments": "" } }),
        json!({ "type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 1, "delta": "{\"city\":" }),
        json!({ "type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 1, "delta": "\"SF\"}" }),
        json!({ "type": "response.output_item.done", "output_index": 1, "item": { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"SF\"}" } }),
        json!({ "type": "response.output_item.added", "output_index": 2, "item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] } }),
        json!({ "type": "response.output_text.delta", "item_id": "msg_1", "output_index": 2, "content_index": 0, "delta": "Hel" }),
        json!({ "type": "response.output_text.delta", "item_id": "msg_1", "output_index": 2, "content_index": 0, "delta": "lo" }),
        json!({ "type": "response.completed", "response": { "id": "resp_1", "model": "grok-4.5", "status": "completed", "object": "response", "output": [], "usage": { "input_tokens": 10, "output_tokens": 20, "output_tokens_details": { "reasoning_tokens": 5 } } } }),
    ];
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse(&chunks), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_input = String::new();
    let mut tool_name = String::new();
    let mut saw_reasoning_end = false;
    let mut saw_tool_input_end = false;
    let mut finish_reason = None;
    let mut finish_usage = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::TextDelta { delta, .. } => text.push_str(&delta),
            LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => reasoning.push_str(&delta),
            LanguageModelV4StreamPart::ReasoningEnd { .. } => saw_reasoning_end = true,
            LanguageModelV4StreamPart::ToolInputDelta { delta, .. } => tool_input.push_str(&delta),
            LanguageModelV4StreamPart::ToolInputEnd { .. } => saw_tool_input_end = true,
            LanguageModelV4StreamPart::ToolCall(tc) => {
                tool_name = tc.tool_name.clone();
                // The final ToolCall carries the fully-assembled arguments.
                assert_eq!(tc.tool_call_id, "call_1");
                let parsed: Value = serde_json::from_str(&tc.input).expect("valid json args");
                assert_eq!(parsed, json!({ "city": "SF" }));
            }
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
    assert_eq!(reasoning, "think");
    assert!(saw_reasoning_end);
    assert!(saw_tool_input_end);
    assert_eq!(tool_name, "get_weather");
    // Streamed via function_call_arguments.delta events.
    assert_eq!(tool_input, "{\"city\":\"SF\"}");

    let usage = finish_usage.expect("finish usage");
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.output_tokens.total, Some(20));
    assert_eq!(usage.output_tokens.text, Some(15));
    assert_eq!(usage.output_tokens.reasoning, Some(5));
    assert_eq!(
        finish_reason.expect("finish reason").unified,
        UnifiedFinishReason::ToolUse
    );
}

#[tokio::test]
async fn do_stream_malformed_chunk_emits_error_and_other_finish() {
    let server = MockServer::start().await;
    let sse_body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hi\"}\n\n",
        "data: {not valid json}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
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
    assert_eq!(fr.unified, UnifiedFinishReason::Other);
    assert_eq!(fr.raw.as_deref(), Some("error"));
}

#[tokio::test]
async fn do_stream_error_event_surfaces_error_part() {
    let server = MockServer::start().await;
    let chunks = vec![
        json!({ "type": "response.created", "response": { "id": "r", "model": "grok-4.5" } }),
        json!({ "type": "error", "code": "server_error", "message": "boom" }),
    ];
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse(&chunks), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut error_message = None;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::Error { error } => error_message = Some(error.message),
            LanguageModelV4StreamPart::Finish {
                finish_reason: fr, ..
            } => finish_reason = Some(fr),
            _ => {}
        }
    }
    assert_eq!(error_message.as_deref(), Some("boom"));
    assert_eq!(
        finish_reason.expect("finish reason").raw.as_deref(),
        Some("error")
    );
}

#[tokio::test]
async fn do_stream_surfaces_200_json_error_body_retryable() {
    let server = MockServer::start().await;
    let body = json!({
        "code": "The service is currently unavailable",
        "error": "grok is warming up",
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(body),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let err = model
        .do_stream(&options(), None)
        .await
        .expect_err("200-with-JSON-error must surface as an error");
    assert_eq!(retryable_of(&err), Some(true));
}

#[tokio::test]
async fn do_stream_200_json_error_body_not_retryable() {
    let server = MockServer::start().await;
    let body = json!({ "code": "boom", "error": "The service is currently unavailable" });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(body),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let err = model
        .do_stream(&options(), None)
        .await
        .expect_err("200-with-JSON-error must surface as an error");
    assert_eq!(retryable_of(&err), Some(false));
    assert!(err.message.contains("The service is currently unavailable"));
}

#[tokio::test]
async fn do_stream_response_failed_maps_to_error_finish() {
    // `response.failed` is a *server-declared* failure: the TS maps a
    // missing/unmappable reason to unified `error` (raw "error"). An `Error`
    // stream part also surfaces the message (coco addition, matching the
    // openai Rust model). Distinct from a malformed chunk, which stays Other.
    let server = MockServer::start().await;
    let chunks = vec![
        json!({ "type": "response.created", "response": { "id": "resp_f", "model": "grok-4.5", "created_at": 1_700_000_000 } }),
        json!({
            "type": "response.failed",
            "response": {
                "id": "resp_f",
                "status": "failed",
                "error": { "code": "server_error", "message": "generation failed" },
                "usage": { "input_tokens": 5, "output_tokens": 0 }
            }
        }),
    ];
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&chunks)),
        )
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
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

    assert!(saw_error, "response.failed must surface an Error part");
    let fr = finish_reason.expect("finish reason");
    assert_eq!(
        fr.unified,
        UnifiedFinishReason::Error,
        "server-declared failure maps to unified Error (TS parity)"
    );
    assert_eq!(fr.raw.as_deref(), Some("error"));
}

#[tokio::test]
async fn do_generate_response_body_echoes_the_response() {
    // `LanguageModelV4Response.body` must carry the parsed *response* (the TS
    // returns rawResponse), not an echo of the request body.
    let server = MockServer::start().await;
    let body = json!({
        "id": "resp_body",
        "object": "response",
        "created_at": 1_700_000_000,
        "status": "completed",
        "model": "grok-4.5",
        "output": [{
            "type": "message",
            "id": "msg_1",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Hi!" }]
        }],
        "usage": { "input_tokens": 3, "output_tokens": 2 }
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.responses("grok-4.5");
    let result = model
        .do_generate(&options(), None)
        .await
        .expect("do_generate");

    let response = result.response.expect("response metadata");
    let echoed = response.body.expect("response body echoed");
    assert_eq!(
        echoed["id"], "resp_body",
        "body is the RESPONSE, not the request"
    );
    assert!(
        echoed.get("input").is_none(),
        "request fields must not leak into the response body"
    );
    let request = result.request.expect("request metadata");
    assert!(request.body.expect("request body").get("input").is_some());
}
