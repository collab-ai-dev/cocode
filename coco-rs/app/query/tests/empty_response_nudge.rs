//! P0.3 — empty-response nudge/retry (hermes absorption).
//!
//! A clean stop with neither text nor tool calls (thinking-only counts)
//! must nudge and retry instead of silently ending the turn, capped at
//! three nudges per user cycle, with `loop.empty_response_nudge = "off"`
//! restoring the legacy end-turn behavior.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod mock_harness;

use std::sync::Arc;

use coco_inference::LanguageModel;
use coco_query::{QueryEngine, QueryEngineConfig, QueryResult};
use coco_types::PermissionMode;
use mock_harness::{MockModelBuilder, MockResponse, core_tools, run_with_mock};
use tokio_util::sync::CancellationToken;

const NUDGE_MARKER: &str = "empty response";

/// Count nudge meta messages in the final transcript. Attachment bodies
/// are matched via `Debug` formatting — the nudge wording is unique.
fn count_nudges(result: &QueryResult) -> usize {
    result
        .final_messages
        .iter()
        .filter(|m| {
            matches!(m.as_ref(), coco_messages::Message::Attachment(_))
                && format!("{m:?}").contains(NUDGE_MARKER)
        })
        .count()
}

async fn run_with_mock_and_policy(
    model: Arc<dyn LanguageModel>,
    prompt: &str,
    policy: coco_config::EmptyResponsePolicy,
) -> QueryResult {
    let client = coco_query::test_support::model_runtime_registry(model);
    let config = QueryEngineConfig {
        model_id: "scripted-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        empty_response_nudge: policy,
        ..Default::default()
    };
    let engine = QueryEngine::new(
        config,
        coco_types::SessionId::try_new("test-session").unwrap(),
        client,
        core_tools(),
        CancellationToken::new(),
        None,
    );
    engine
        .run(prompt)
        .await
        .expect("mock engine should not fail")
}

#[tokio::test]
async fn empty_then_text_gets_one_nudge_and_completes() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text(""))
        .on_call(1, |_| MockResponse::text("Recovered answer"))
        .build();
    let result = run_with_mock(model, "hello", core_tools()).await;
    assert_eq!(result.response_text, "Recovered answer");
    assert_eq!(count_nudges(&result), 1, "exactly one nudge injected");
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
}

#[tokio::test]
async fn persistent_empty_caps_at_three_nudges_then_ends_cleanly() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text(""))
        .on_call(1, |_| MockResponse::text(""))
        .on_call(2, |_| MockResponse::text(""))
        .on_call(3, |_| MockResponse::text(""))
        .on_call(4, |_| MockResponse::text("never reached"))
        .build();
    let result = run_with_mock(model, "hello", core_tools()).await;
    // Three nudges fired, the fourth empty response ends the turn — no
    // infinite loop, no fifth model call.
    assert_eq!(count_nudges(&result), 3);
    assert_eq!(result.response_text, "");
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
    assert!(
        !result
            .final_messages
            .iter()
            .any(|m| format!("{m:?}").contains("never reached")),
        "the loop must stop after the cap, not keep polling the model"
    );
}

#[tokio::test]
async fn reasoning_only_response_is_nudged_and_never_leaks_thinking() {
    // Anti-lesson 9 (hermes v0.19): after the thinking-only retry was
    // added, `<think>` content leaked into user-visible output. Pin that
    // reasoning text never reaches visible output on the nudge path.
    const THINKING: &str = "secret chain of thought about the plan";
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::reasoning_only(THINKING))
        .on_call(1, |_| MockResponse::text("Visible answer"))
        .build();
    let result = run_with_mock(model, "hello", core_tools()).await;
    assert_eq!(count_nudges(&result), 1, "thinking-only counts as empty");
    assert_eq!(result.response_text, "Visible answer");
    assert!(
        !result.response_text.contains(THINKING),
        "reasoning must never leak into visible output"
    );
    for m in &result.final_messages {
        if let coco_messages::Message::User(u) = m.as_ref() {
            let text = format!("{:?}", u.message);
            assert!(
                !text.contains(THINKING),
                "reasoning must not be replayed through user-role scaffolding"
            );
        }
    }
}

#[tokio::test]
async fn off_policy_keeps_legacy_end_turn() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text(""))
        .on_call(1, |_| MockResponse::text("never reached"))
        .build();
    let result =
        run_with_mock_and_policy(model, "hello", coco_config::EmptyResponsePolicy::Off).await;
    assert_eq!(count_nudges(&result), 0);
    assert_eq!(result.response_text, "");
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
}

#[tokio::test]
async fn empty_after_tool_use_gets_post_tool_wording() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::tool_call("Bash", serde_json::json!({"command": "echo hi"}))
        })
        .on_call(1, |_| MockResponse::text(""))
        .on_call(2, |_| MockResponse::text("Processed the results"))
        .build();
    let result = run_with_mock(model, "run echo", core_tools()).await;
    assert_eq!(result.response_text, "Processed the results");
    let post_tool_nudges = result
        .final_messages
        .iter()
        .filter(|m| {
            matches!(m.as_ref(), coco_messages::Message::Attachment(_))
                && format!("{m:?}").contains("process the tool results above")
        })
        .count();
    assert_eq!(
        post_tool_nudges, 1,
        "post-tool empty response uses the tool-aware wording"
    );
}
