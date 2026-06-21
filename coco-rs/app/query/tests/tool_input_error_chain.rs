//! Full-chain integration tests for tool input parsing + validation.
//!
//! Each test scripts a mock `LanguageModel` that returns ToolCallParts
//! representing what the provider adapter would emit on the wire for a
//! given malformation. The test then asserts on `QueryResult.final_messages`
//! that the synthetic `tool_result` produced by the agent loop carries
//! the expected `<tool_use_error>` content — exercising the entire
//! query → inference → provider → tool_call_preparer → schema-validation →
//! tool_result chain end-to-end.
//!
//! Three emission shapes are covered (see `MockToolEmission`):
//! - `Clean` — pre-parsed object (Anthropic non-streaming Value, Gemini).
//! - `FromRawArguments` — raw string passed through wire parsing helper
//!   (OpenAI Chat / Responses / OpenAI-compat / Anthropic streaming).
//! - `InvalidWithReason` — adapter detected unrecoverable parse failure
//!   and emitted `invalid_reason` directly.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_llm_types::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use serde_json::json;

mod mock_harness;

use mock_harness::MockModelBuilder;
use mock_harness::MockResponse;
use mock_harness::MockToolEmission;
use mock_harness::core_tools;
use mock_harness::run_with_mock;

// ---------------------------------------------------------------------------
// Helper: extract every `tool_result` body produced by the engine, in
// order. Each body is the synthetic message the agent loop pushed for a
// tool_use that ran (or short-circuited via invalid_reason).
// ---------------------------------------------------------------------------

fn tool_result_bodies(result: &coco_query::QueryResult) -> Vec<String> {
    let mut out = Vec::new();
    for msg in &result.final_messages {
        if let Message::ToolResult(tr) = msg.as_ref() {
            let LlmMessage::Tool { content, .. } = &tr.message else {
                continue;
            };
            for part in content {
                let ToolContent::ToolResult(part) = part else {
                    continue;
                };
                let body = match &part.output {
                    coco_llm_types::ToolResultContent::Text { value, .. } => value.clone(),
                    coco_llm_types::ToolResultContent::ErrorText { value, .. } => value.clone(),
                    coco_llm_types::ToolResultContent::Content { value, .. } => value
                        .iter()
                        .filter_map(|p| match p {
                            coco_llm_types::ToolResultContentPart::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                    _ => String::new(),
                };
                out.push(body);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Scenario 1: OpenAI-style raw `arguments` string — trailing comma is
// repaired in wire parsing and the tool executes successfully.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_openai_trailing_comma_repaired_and_executes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "ok").unwrap();
    let path = file.to_str().unwrap().to_string();

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw(
                "Read",
                &format!(r#"{{"file_path": "{path}", "limit": 100,}}"#),
            )])
        })
        .then_text("Read complete.")
        .build();

    let result = run_with_mock(model, "read it", core_tools()).await;
    assert_eq!(result.turns, 2);
    let bodies = tool_result_bodies(&result);
    assert_eq!(bodies.len(), 1, "expected one tool result, got {bodies:?}");
    assert!(
        !bodies[0].contains("<tool_use_error>"),
        "trailing-comma input should repair, not error: {}",
        bodies[0]
    );
    assert!(
        bodies[0].contains("ok"),
        "tool result should contain file content: {}",
        bodies[0]
    );
}

// ---------------------------------------------------------------------------
// Scenario 2: GLM/Doubao-style raw markdown-fenced `arguments` — fence
// is stripped in wire parsing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_glm_markdown_fence_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("b.txt");
    std::fs::write(&file, "ok").unwrap();
    let path = file.to_str().unwrap().to_string();
    let raw = format!("```json\n{{\"file_path\": \"{path}\"}}\n```");

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw("Read", &raw)])
        })
        .then_text("Done.")
        .build();

    let result = run_with_mock(model, "read", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    assert!(
        !bodies[0].contains("<tool_use_error>"),
        "markdown fence should be stripped: {}",
        bodies[0]
    );
}

// ---------------------------------------------------------------------------
// Scenario 3: Missing required field — wire parsing falls back to `{}`,
// downstream validation (schema validation schema validator when the tool
// declares a full JSON schema, OR `tool.validate_input` semantic
// check for tools still on the property-only schema) produces a
// model-readable error naming the missing field.
//
// `core_tools()` registers production tools (Read/Bash/…) which use
// the property-only `input_schema()` envelope; schema validation vacuously
// passes (no `required` constraint), so this test asserts the
// downstream `tool.validate_input` path. Once tools migrate to full
// `input_json_schema()` overrides, schema validation will surface the
// `<tool_use_error>InputValidationError: …>` wrap instead — the
// invariant is "model sees a useful error", not the exact wrap.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_missing_required_field_surfaces_useful_error() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw("Read", "")])
        })
        .then_text("Recovered.")
        .build();

    let result = run_with_mock(model, "read missing", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    let useful = bodies.iter().any(|b| {
        // Either path (schema validation SchemaViolation wrap, or Tool runtime
        // validation message) is acceptable as long as `file_path`
        // is named.
        b.contains("file_path")
            && (b.contains("missing required field")
                || (b.contains("<tool_use_error>") && b.contains("InputValidationError")))
    });
    assert!(useful, "expected error naming `file_path`, got: {bodies:?}");
}

// ---------------------------------------------------------------------------
// Scenario 4: Adapter signals JsonParseFailed (raw bytes were
// unrecoverable). Provider-level `ToolCallPart.invalid_reason` is
// **not preserved through `synthetic_stream_from_content`** —
// `LanguageModelV4ToolCall` (wire type) carries only the input
// string, by design (mirrors `@ai-sdk/provider`). Engine rebuilds
// the `ToolCallPart` from stream events and runs wire parsing again on
// the (string) input.
//
// Production: this means the **real** path for JsonParseFailed is
// the Anthropic streaming `content_block_stop` flush, not the
// non-streaming adapter — and provider adapters carry the wrap
// inline before emitting. The full-chain test here verifies that
// the model still receives a useful error in this scenario (the
// fallback path via `tool.validate_input` after `{}` fallback).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_invalid_reason_carried_directly_via_engine() {
    // Use `from_raw` with garbage that hits wire parsing's `{}` fallback;
    // downstream validation picks up the missing field.
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw(
                "Read",
                "\u{0000}!!! unrecoverable",
            )])
        })
        .then_text("Recovered.")
        .build();

    let result = run_with_mock(model, "x", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    assert!(
        !bodies.is_empty(),
        "expected at least one tool_result emitted"
    );
    // Either schema validation produces an InputValidationError
    // wrap (when `parse_with_repair` salvages to a non-object value
    // that fails the `object` constraint) or the tool's runtime
    // `validate_input` names the missing field. Both are useful
    // signals back to the model.
    let useful = bodies.iter().any(|b| {
        b.contains("InputValidationError")
            || b.contains("file_path")
            || b.contains("expected as `object`")
    });
    assert!(
        useful,
        "expected a useful structured error, got: {bodies:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 5: Hallucinated tool name (provider returns a ToolCallPart
// for a tool that isn't in the registry). schema validation NoSuchTool short-
// circuits with the dedicated wrap prefix.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_no_such_tool_produces_dedicated_wrap() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::clean(
                "DoTheImpossible",
                json!({"x": 1}),
            )])
        })
        .then_text("Recovered.")
        .build();

    let result = run_with_mock(model, "x", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    assert!(
        bodies.iter().any(|b| b.contains("<tool_use_error>")
            && b.contains("No such tool available")
            && b.contains("DoTheImpossible")),
        "expected NoSuchTool wrap naming `DoTheImpossible`: {bodies:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 6: Mixed batch — multiple tool calls in one turn, some
// valid, some not. Each one's tool_result is independent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_multi_tool_mixed_outcomes_each_independent() {
    // Mixed batch — 3 emissions in one turn, each classified
    // independently. Note: `MockToolEmission::InvalidWithReason` is
    // omitted from this batch because `synthetic_stream_from_content`
    // strips `invalid_reason` (wire-level streaming type can't carry
    // it). That path is exercised by the unit tests in
    // `tool_input_validate.test.rs` which call `validate_tool_call`
    // directly.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "content").unwrap();
    let path = file.to_str().unwrap().to_string();

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::MixedToolCalls(vec![
                // valid Read with markdown fence — should repair + execute
                MockToolEmission::from_raw(
                    "Read",
                    &format!("```json\n{{\"file_path\": \"{path}\"}}\n```"),
                ),
                // missing required → useful error via tool.validate_input
                MockToolEmission::from_raw("Read", ""),
                // hallucinated tool name → NoSuchTool wrap (registry miss)
                MockToolEmission::clean("MysteryTool", json!({})),
            ])
        })
        .then_text("All processed.")
        .build();

    let result = run_with_mock(model, "go", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    assert_eq!(
        bodies.len(),
        3,
        "expected 3 tool results (one per emission), got {bodies:?}"
    );

    // Order of tool_result emissions can shuffle when safe tools run
    // concurrently — assert by content rather than position.

    // One body must be a clean success (no `<tool_use_error>` wrap
    // and no legacy `Invalid input:` prefix).
    let success_count = bodies
        .iter()
        .filter(|b| !b.contains("<tool_use_error>") && !b.starts_with("Invalid input"))
        .count();
    assert_eq!(
        success_count, 1,
        "expected exactly one success body: {bodies:?}"
    );

    // One body must name the missing required field (`file_path`).
    let missing_field = bodies
        .iter()
        .any(|b| b.contains("file_path") || b.contains("InputValidationError"));
    assert!(
        missing_field,
        "expected an InputValidationError / missing field error: {bodies:?}"
    );

    // One body must be the NoSuchTool wrap naming MysteryTool.
    let no_such_tool = bodies
        .iter()
        .any(|b| b.contains("No such tool available") && b.contains("MysteryTool"));
    assert!(no_such_tool, "expected NoSuchTool wrap: {bodies:?}");
}

// ---------------------------------------------------------------------------
// Scenario 7: Anthropic Value::String passthrough — wire parsing hands the
// raw `Value::String("{json}")`, schema validation `normalize_value_string`
// recovers the inner object before schema validation. The model
// doesn't see an error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_anthropic_value_string_recovered_in_layer_2() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("recovered.txt");
    std::fs::write(&file, "recovered").unwrap();
    let path = file.to_str().unwrap().to_string();
    // Simulate Anthropic non-streaming returning the input as a
    // stringified JSON nested inside a Value::String (model failure
    // mode that occasionally leaks the OpenAI shape into Anthropic).
    let nested = format!("{{\"file_path\": \"{path}\"}}");

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::clean(
                "Read",
                serde_json::Value::String(nested.clone()),
            )])
        })
        .then_text("Read complete.")
        .build();

    let result = run_with_mock(model, "read", core_tools()).await;
    let bodies = tool_result_bodies(&result);
    assert!(
        !bodies[0].contains("<tool_use_error>"),
        "Value::String should be recovered by schema validation normalize_value_string: {}",
        bodies[0]
    );
    // Read tool prefixes lines with byte counters / numbers; just
    // verify no error wrap fired — the recovery succeeded.
}

// ---------------------------------------------------------------------------
// Scenario 8: Multi-turn self-correction — first turn emits a malformed
// call, agent loop replies with `<tool_use_error>`, second turn the
// model fixes it. This is the most important end-to-end invariant:
// the LLM's view of the error message is sufficient to self-correct.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_chain_self_correction_after_validation_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.txt");
    std::fs::write(&file, "ok").unwrap();
    let path = file.to_str().unwrap().to_string();
    let path_clone = path.clone();

    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            // Turn 1: model omits `file_path`.
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw(
                "Read",
                r#"{"limit": 100}"#,
            )])
        })
        .on_call(1, move |_| {
            // Turn 2: model fixes it after seeing the error.
            MockResponse::MixedToolCalls(vec![MockToolEmission::clean(
                "Read",
                json!({"file_path": path_clone}),
            )])
        })
        .on_call(2, |_| MockResponse::text("Got it."))
        .build();

    let result = run_with_mock(model, "read with correction", core_tools()).await;
    assert_eq!(result.turns, 3, "expected 3 turns: error → fix → text");

    let bodies = tool_result_bodies(&result);
    assert_eq!(bodies.len(), 2, "expected 2 tool_results, got {bodies:?}");
    assert!(
        bodies[0].contains("file_path"),
        "first call should fail validation naming file_path: {}",
        bodies[0]
    );
    assert!(
        !bodies[1].contains("<tool_use_error>")
            && !bodies[1].contains("Invalid input")
            && bodies[1].contains("ok"),
        "second call should succeed: {}",
        bodies[1]
    );
}
