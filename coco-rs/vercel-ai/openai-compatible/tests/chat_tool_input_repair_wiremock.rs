//! Wire-level repair tests for OpenAI-compatible Chat Completions.
//!
//! Same pattern as `vercel-ai-openai`'s `chat_tool_input_repair_wiremock`
//! — covers the matrix of malformations encountered against
//! OpenAI-compat endpoints (GLM, Doubao, DeepSeek, Groq, xAI, Ollama)
//! which is the family where messy `arguments` strings most often
//! show up in practice.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vercel_ai_openai_compatible::OpenAICompatibleProviderSettings;
use vercel_ai_openai_compatible::create_openai_compatible;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn chat_response_with_arguments(arguments: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-compat",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": "compat-test",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_compat",
                    "type": "function",
                    "function": {
                        "name": "Read",
                        "arguments": arguments,
                    },
                }],
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
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

async fn dispatch(raw_arguments: &str) -> vercel_ai_provider::ToolCallPart {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(chat_response_with_arguments(raw_arguments)),
        )
        .mount(&server)
        .await;

    let provider = create_openai_compatible(OpenAICompatibleProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        name: Some("compat".into()),
        ..Default::default()
    });
    let model = provider.chat("compat-test");
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

#[tokio::test]
async fn compat_nonstream_clean() {
    let tc = dispatch(r#"{"file_path": "/tmp/x"}"#).await;
    assert_eq!(tc.input, json!({"file_path": "/tmp/x"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_nonstream_glm_markdown_fence_repaired() {
    // GLM-4 / Doubao-pro / DeepSeek frequently wrap arguments in
    // ```json...``` despite the OpenAI-compat protocol forbidding it.
    let tc = dispatch("```json\n{\"file_path\": \"/r/main.rs\"}\n```").await;
    assert_eq!(tc.input, json!({"file_path": "/r/main.rs"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_nonstream_trailing_comma_repaired() {
    let tc = dispatch(r#"{"file_path": "/tmp/x", "limit": 100,}"#).await;
    assert_eq!(tc.input, json!({"file_path": "/tmp/x", "limit": 100}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_nonstream_deepseek_single_quotes_repaired() {
    let tc = dispatch(r#"{'file_path': '/tmp/sq'}"#).await;
    assert_eq!(tc.input, json!({"file_path": "/tmp/sq"}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_nonstream_empty_arguments_becomes_empty_object() {
    let tc = dispatch("").await;
    assert_eq!(tc.input, json!({}));
    assert!(!tc.invalid);
}

#[tokio::test]
async fn compat_nonstream_unrecoverable_layer_1_does_not_invalidate() {
    let tc = dispatch("\u{0000}!!!@@@%%%").await;
    assert_ne!(tc.input, serde_json::json!({}));
    assert!(!tc.invalid);
}
