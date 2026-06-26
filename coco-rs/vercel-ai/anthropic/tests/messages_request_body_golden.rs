//! Golden snapshots of the serialized Anthropic Messages request body.
//!
//! Ported from opencode's `cache-policy.test.ts` philosophy: instead of
//! re-deriving the *expected* wire shape in the test (which drifts from the
//! real serializer), we drive the real `get_args` serializer and snapshot the
//! actual outbound body + the provider-sensitive headers. Any change to the
//! request the model SENDS — system-block layout, tool-schema shape, the
//! `stream` flag, cache-control placement, or the `anthropic-beta` header set
//! — shows up as a reviewable snapshot diff instead of passing silently.
//!
//! This is the request-side counterpart to the `*_wiremock.rs` tests, which
//! only exercise the response side and match requests on method+path alone.
//!
//! Regenerate after an intentional wire change with:
//!   INSTA_UPDATE=always cargo test -p vercel-ai-anthropic \
//!     --test messages_request_body_golden
//! then review the snapshot diff before committing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vercel_ai_anthropic::AnthropicProviderSettings;
use vercel_ai_anthropic::create_anthropic;
use vercel_ai_anthropic::messages::AnthropicMessagesLanguageModel;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use vercel_ai_provider::language_model::v4::LanguageModelV4FunctionTool;

/// Build a model with deterministic settings — fixed base_url + fake key so
/// the serialized body and header set are byte-stable across runs and hosts.
fn model() -> AnthropicMessagesLanguageModel {
    create_anthropic(AnthropicProviderSettings {
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    })
    .messages("claude-sonnet-4-5")
}

/// Snapshot view: the full request body plus the two provider-sensitive
/// headers. Credentials (`x-api-key`, `authorization`) are deliberately
/// excluded so no key — even a fake one — lands in a committed snapshot.
fn wire_view(options: &LanguageModelV4CallOptions, stream: bool) -> serde_json::Value {
    let (body, headers, _warnings) = model()
        .get_args(options, stream)
        .unwrap_or_else(|e| panic!("get_args should succeed: {e}"));
    json!({
        "body": body,
        "headers": {
            "anthropic-beta": headers.get("anthropic-beta"),
            "anthropic-version": headers.get("anthropic-version"),
        },
    })
}

#[test]
fn golden_simple_text_request() {
    let options = LanguageModelV4CallOptions::new(vec![LanguageModelV4Message::user_text(
        "Summarize the repo.",
    )]);
    insta::assert_json_snapshot!("simple_text", wire_view(&options, false));
}

#[test]
fn golden_streaming_request_sets_stream_flag_and_beta() {
    let options =
        LanguageModelV4CallOptions::new(vec![LanguageModelV4Message::user_text("Stream a reply.")]);
    // Streaming must carry `stream: true` and the fine-grained-tool-streaming
    // beta; the snapshot pins both.
    insta::assert_json_snapshot!("streaming", wire_view(&options, true));
}

#[test]
fn golden_function_tool_schema_shape() {
    let read_tool = LanguageModelV4Tool::Function(LanguageModelV4FunctionTool::with_description(
        "Read",
        "Read a file from disk",
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path" },
                "limit": { "type": "integer", "description": "Max lines" }
            },
            "required": ["file_path"],
            "additionalProperties": false
        }),
    ));
    let options =
        LanguageModelV4CallOptions::new(vec![LanguageModelV4Message::user_text("read /tmp/x")])
            .with_tools(vec![read_tool]);
    // Pins the function-tool wire shape (name/description/input_schema) — the
    // surface most prone to silent drift across SDK bumps.
    insta::assert_json_snapshot!("function_tool", wire_view(&options, false));
}

#[test]
fn golden_system_block_and_temperature_clamp() {
    let options = LanguageModelV4CallOptions::new(vec![
        LanguageModelV4Message::System {
            content: vec![UserContentPart::Text(TextPart::new("You are coco."))],
            provider_options: None,
        },
        LanguageModelV4Message::user_text("Hi"),
    ])
    .with_temperature(2.0);
    // System-block layout plus temperature clamping (2.0 -> 1.0) are both
    // load-bearing wire behaviors; freeze them together.
    insta::assert_json_snapshot!("system_and_temperature", wire_view(&options, false));
}
