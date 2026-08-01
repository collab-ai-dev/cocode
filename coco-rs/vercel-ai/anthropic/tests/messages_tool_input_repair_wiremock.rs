//! Wire-level tool-input handling tests for Anthropic Messages API
//! (non-streaming).
//!
//! Anthropic's wire shape is **fundamentally different** from
//! OpenAI's: `tool_use.input` is a JSON object directly (not a
//! stringified `arguments` field). Adapter passes the value through
//! verbatim, which means there's no wire-parsing string repair here.
//! Two paths are covered:
//!
//! 1. **Happy path** — `input: {…}` (Value::Object) is preserved
//!    exactly, no transformation.
//! 2. **Value::String anomaly** — when the model nests stringified
//!    JSON inside `input` (an occasional cross-protocol leak), wire parsing
//!    still passes the string through. The recovery is schema validation's
//!    job (`normalize_value_string` in `app/query`). This test pins
//!    the wire parsing contract: the string is **not silently mangled**
//!    at the wire boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vercel_ai_anthropic::AnthropicProviderSettings;
use vercel_ai_anthropic::create_anthropic;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::ToolDefinitionV4;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn messages_response_with_input(input: serde_json::Value) -> serde_json::Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-test",
        "content": [{
            "type": "tool_use",
            "id": "toolu_abc",
            "name": "Read",
            "input": input,
        }],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
        },
    })
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

async fn dispatch(input: serde_json::Value) -> vercel_ai_provider::ToolCallPart {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(messages_response_with_input(input)))
        .mount(&server)
        .await;

    let provider = create_anthropic(AnthropicProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.messages("claude-test");
    let options = one_shot_options();

    let result = model
        .do_generate(&options, None)
        .await
        .expect("do_generate against wiremock should succeed");

    result
        .content
        .into_iter()
        .find_map(|p| match p {
            AssistantContentPart::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .expect("response contained a tool call by construction")
}

fn projected_tool(name: &str) -> LanguageModelV4Tool {
    LanguageModelV4Tool::Function(ToolDefinitionV4 {
        name: name.into(),
        description: None,
        input_schema: json!({
            "type": "object",
            "properties": {
                "file path": {"type": "string"}
            },
            "required": ["file path"]
        }),
        input_examples: None,
        strict: None,
        provider_options: None,
    })
}

async fn projected_nonstream_call(
    response_input: serde_json::Value,
) -> (vercel_ai_provider::ToolCallPart, serde_json::Value, String) {
    let server = MockServer::start().await;
    let provider = create_anthropic(AnthropicProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.messages("claude-test");
    let mut options = one_shot_options();
    options.tools = Some(vec![projected_tool("Read")]);
    let (prepared, _, _) = model
        .get_args(&options, false)
        .expect("prepare Anthropic request");
    let wire_name = prepared["tools"][0]["input_schema"]["properties"]
        .as_object()
        .expect("projected properties")
        .keys()
        .next()
        .expect("wire property")
        .clone();

    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(messages_response_with_input(response_input)),
        )
        .mount(&server)
        .await;
    let result = model
        .do_generate(&options, None)
        .await
        .expect("do_generate against wiremock should succeed");
    let request = server
        .received_requests()
        .await
        .expect("recorded requests")
        .into_iter()
        .next()
        .expect("Anthropic request");
    let request_body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("JSON request body");
    let tool_call = result
        .content
        .into_iter()
        .find_map(|part| match part {
            AssistantContentPart::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("response tool call");
    (tool_call, request_body, wire_name)
}

#[tokio::test]
async fn anthropic_nonstream_object_input_round_trips() {
    let tc = dispatch(json!({"file_path": "/tmp/x", "limit": 100})).await;
    assert_eq!(tc.tool_name, "Read");
    assert_eq!(tc.input, json!({"file_path": "/tmp/x", "limit": 100}));
    assert!(!tc.invalid);
    assert!(tc.invalid_reason.is_none());
}

#[tokio::test]
async fn anthropic_nonstream_empty_object_input_preserved() {
    let tc = dispatch(json!({})).await;
    assert_eq!(tc.input, json!({}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_nonstream_value_string_passes_through_unmangled() {
    // Anthropic returns `input` as `Value` directly, so a stringified
    // JSON nested inside (rare cross-protocol leak from the model)
    // arrives at the adapter as a `Value::String`. wire parsing does NOT
    // recursively parse — `normalize_value_string` at schema validation
    // (`app/query::tool_input_validate`) handles that case. Test
    // pins the wire parsing contract: the raw string survives intact.
    let nested = "{\"file_path\":\"/tmp/recovered\"}";
    let tc = dispatch(json!(nested)).await;
    assert_eq!(tc.input, json!(nested));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_nonstream_value_string_with_markdown_fence_passes_through() {
    // Even more exotic: model emits a markdown-fenced stringified
    // JSON inside `input`. wire parsing doesn't unwrap; schema validation does.
    let fenced = "```json\n{\"file_path\":\"/tmp/fenced\"}\n```";
    let tc = dispatch(json!(fenced)).await;
    assert_eq!(tc.input, json!(fenced));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_nonstream_null_input_preserved() {
    // Per wire spec input must be an object, but Anthropic
    // occasionally emits `null` on degenerate completions. wire parsing
    // passes through — schema validation catches the type
    // mismatch with `InputValidationError`.
    let tc = dispatch(serde_json::Value::Null).await;
    assert!(matches!(tc.input, serde_json::Value::Null));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn anthropic_nonstream_projects_request_and_restores_response() {
    let server = MockServer::start().await;
    let provider = create_anthropic(AnthropicProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.messages("claude-test");
    let mut options = one_shot_options();
    options.tools = Some(vec![projected_tool("Read")]);
    let (prepared, _, _) = model
        .get_args(&options, false)
        .expect("prepare Anthropic request");
    let wire_name = prepared["tools"][0]["input_schema"]["properties"]
        .as_object()
        .expect("projected properties")
        .keys()
        .next()
        .expect("wire property")
        .clone();
    assert_ne!(wire_name, "file path");

    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(messages_response_with_input(
                json!({(wire_name.clone()): "/tmp/x"}),
            )),
        )
        .mount(&server)
        .await;
    let result = model
        .do_generate(&options, None)
        .await
        .expect("do_generate against wiremock should succeed");
    let request = server
        .received_requests()
        .await
        .expect("recorded requests")
        .into_iter()
        .next()
        .expect("Anthropic request");
    let request_body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("JSON request body");
    assert!(
        request_body["tools"][0]["input_schema"]["properties"]
            .get(&wire_name)
            .is_some()
    );
    assert!(
        request_body["tools"][0]["input_schema"]["properties"]
            .get("file path")
            .is_none()
    );
    let call = result
        .content
        .into_iter()
        .find_map(|part| match part {
            AssistantContentPart::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("response tool call");
    assert_eq!(call.input, json!({"file path": "/tmp/x"}));
    assert!(!call.invalid);
}

#[tokio::test]
async fn anthropic_nonstream_alias_collision_is_invalid_and_atomic() {
    let (probe, _, wire_name) = projected_nonstream_call(json!({})).await;
    assert!(!probe.invalid);
    let collision = json!({
        "file path": "original",
        (wire_name.clone()): "projected"
    });
    let (call, _, _) = projected_nonstream_call(collision.clone()).await;
    assert!(call.invalid);
    assert_eq!(call.input, collision, "failed restoration must be atomic");
    assert!(matches!(
        call.invalid_reason,
        Some(vercel_ai_provider::ToolInputInvalidReason::SchemaViolation { .. })
    ));
}

#[tokio::test]
async fn anthropic_buffered_json_response_restores_projected_names() {
    let server = MockServer::start().await;
    let provider = create_anthropic(AnthropicProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".to_string()),
        supports_native_structured_output: Some(false),
        ..Default::default()
    });
    let model = provider.messages("claude-test");
    let options = one_shot_options().with_response_format(ResponseFormat::Json {
        schema: Some(json!({
            "type": "object",
            "properties": {"result value": {"type": "string"}},
            "required": ["result value"]
        })),
        name: None,
        description: None,
    });
    let (prepared, _, _) = model
        .get_args(&options, false)
        .expect("prepare JSON response tool");
    let wire_name = prepared["tools"][0]["input_schema"]["properties"]
        .as_object()
        .expect("projected JSON schema properties")
        .keys()
        .next()
        .expect("wire property")
        .clone();

    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_json",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [{
                "type": "tool_use",
                "id": "toolu_json",
                "name": "json",
                "input": {(wire_name): "restored"}
            }],
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 5}
        })))
        .mount(&server)
        .await;
    let result = model
        .do_generate(&options, None)
        .await
        .expect("buffered JSON response");
    let text = result
        .content
        .into_iter()
        .find_map(|part| match part {
            AssistantContentPart::Text(text) => Some(text.text),
            _ => None,
        })
        .expect("JSON response text");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).expect("JSON response"),
        json!({"result value": "restored"})
    );
}
