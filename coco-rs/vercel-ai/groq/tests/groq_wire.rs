//! Wire-level tests for the Groq chat model: `do_generate` response parsing
//! and `do_stream` with usage delivered via `x_groq.usage`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::StreamExt;
use serde_json::json;
use vercel_ai_groq::GroqProviderSettings;
use vercel_ai_groq::create_groq;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::UnifiedFinishReason;
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
async fn do_generate_parses_reasoning_tool_calls_and_usage() {
    let server = MockServer::start().await;
    let body = json!({
        "id": "chatcmpl-1",
        "created": 1_700_000_000,
        "model": "llama-3.3-70b-versatile",
        "choices": [{
            "index": 0,
            "message": {
                "content": "Hello!",
                "reasoning": "thinking...",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
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

    let provider = create_groq(GroqProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.chat("llama-3.3-70b-versatile");
    let result = model
        .do_generate(&options(), None)
        .await
        .expect("do_generate");

    // Content order: text, then reasoning, then tool call.
    let mut saw_text = false;
    let mut saw_reasoning = false;
    let mut saw_tool = false;
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
            _ => {}
        }
    }
    assert!(saw_text && saw_reasoning && saw_tool);

    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolUse);
    assert_eq!(result.usage.input_tokens.total(), Some(10));
    assert_eq!(result.usage.output_tokens.total, Some(20));
    assert_eq!(result.usage.output_tokens.text, Some(15));
    assert_eq!(result.usage.output_tokens.reasoning, Some(5));
}

#[tokio::test]
async fn do_stream_emits_deltas_and_x_groq_usage() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"c1\",\"created\":1700000000,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"th\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"x_groq\":{\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"completion_tokens_details\":{\"reasoning_tokens\":5}}}}\n\n",
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

    let provider = create_groq(GroqProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.chat("m");

    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut finish_usage = None;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::TextDelta { delta, .. } => text.push_str(&delta),
            LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => reasoning.push_str(&delta),
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
    let usage = finish_usage.expect("finish usage");
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.output_tokens.total, Some(20));
    assert_eq!(usage.output_tokens.text, Some(15));
    assert_eq!(usage.output_tokens.reasoning, Some(5));
    assert_eq!(
        finish_reason.expect("finish reason").unified,
        UnifiedFinishReason::EndTurn
    );
}

#[tokio::test]
async fn do_stream_assembles_tool_call_across_deltas() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"SF\\\"}\"}}]}}]}\n\n",
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

    let provider = create_groq(GroqProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.chat("llama-3.3-70b-versatile");
    let mut stream_result = model.do_stream(&options(), None).await.expect("do_stream");

    let mut tool_call: Option<LanguageModelV4ToolCall> = None;
    let mut finish_reason = None;
    while let Some(part) = stream_result.stream.next().await {
        match part.expect("stream part") {
            LanguageModelV4StreamPart::ToolCall(tc) => tool_call = Some(tc),
            LanguageModelV4StreamPart::Finish {
                finish_reason: fr, ..
            } => finish_reason = Some(fr),
            _ => {}
        }
    }

    let tc = tool_call.expect("a tool call");
    assert_eq!(tc.tool_name, "get_weather");
    assert!(!tc.invalid);
    let parsed: serde_json::Value = serde_json::from_str(&tc.input).expect("valid json args");
    assert_eq!(parsed, json!({"city": "SF"}));
    assert_eq!(
        finish_reason.expect("finish reason").unified,
        UnifiedFinishReason::ToolUse
    );
}
