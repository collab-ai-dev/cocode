//! P0.3 — empty-response nudge/retry (hermes absorption).
//!
//! Empty and malformed tool-use terminals retry through a prompt-only
//! assistant/user overlay. Recovery scaffolding must never persist.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod mock_harness;

use std::sync::Arc;

use coco_inference::LanguageModel;
use coco_query::{QueryEngine, QueryEngineConfig, QueryResult};
use coco_types::PermissionMode;
use mock_harness::{MockModelBuilder, MockResponse, core_tools, run_with_mock};
use tokio_util::sync::CancellationToken;

const NUDGE_MARKER: &str = "empty response";

fn count_persisted_nudges(result: &QueryResult) -> usize {
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
        .on_call(1, |options| {
            assert!(
                format!("{:?}", options.prompt).contains("empty response"),
                "retry prompt must carry the ephemeral nudge"
            );
            MockResponse::text("Recovered answer")
        })
        .build();
    let result = run_with_mock(model, "hello", core_tools()).await;
    assert_eq!(result.response_text, "Recovered answer");
    assert_eq!(
        count_persisted_nudges(&result),
        0,
        "recovery nudge must not enter durable history"
    );
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
}

#[tokio::test]
async fn persistent_empty_caps_at_three_nudges_then_fails() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text(""))
        .on_call(1, |_| MockResponse::text(""))
        .on_call(2, |_| MockResponse::text(""))
        .on_call(3, |_| MockResponse::text(""))
        .on_call(4, |_| MockResponse::text("never reached"))
        .build();
    let result = run_with_mock(model, "hello", core_tools()).await;
    assert_eq!(count_persisted_nudges(&result), 0);
    assert_eq!(result.response_text, "");
    assert_eq!(
        result.stop_reason.as_deref(),
        Some("error_empty_response_retries")
    );
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
    assert_eq!(count_persisted_nudges(&result), 0);
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
    assert_eq!(count_persisted_nudges(&result), 0);
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
        .on_call(2, |options| {
            assert!(
                format!("{:?}", options.prompt).contains("process the tool results above"),
                "post-tool retry must use tool-aware wording"
            );
            MockResponse::text("Processed the results")
        })
        .build();
    let result = run_with_mock(model, "run echo", core_tools()).await;
    assert_eq!(result.response_text, "Processed the results");
    assert_eq!(count_persisted_nudges(&result), 0);
}

#[tokio::test]
async fn tool_use_without_calls_retries_even_when_narration_is_present() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::text_with_stop(
                "I'll inspect the file now.",
                coco_llm_types::StopReason::ToolUse,
            )
        })
        .on_call(1, |options| {
            let prompt = format!("{:?}", options.prompt);
            assert!(prompt.contains("no tool call was provided"));
            assert!(prompt.contains("I'll inspect the file now."));
            MockResponse::text("Recovered without a tool")
        })
        .build();

    let result = run_with_mock(model, "inspect it", core_tools()).await;
    assert_eq!(result.response_text, "Recovered without a tool");
    assert_eq!(count_persisted_nudges(&result), 0);
    assert!(
        !result
            .final_messages
            .iter()
            .any(|message| format!("{message:?}").contains("I'll inspect the file now.")),
        "malformed assistant narration must remain transient"
    );
}

#[tokio::test]
async fn persistent_tool_use_without_calls_fails_after_three_retries() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::text_with_stop("", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(1, |_| {
            MockResponse::text_with_stop("", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(2, |_| {
            MockResponse::text_with_stop("", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(3, |_| {
            MockResponse::text_with_stop("", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(4, |_| MockResponse::text("never reached"))
        .build();

    let result = run_with_mock(model, "use a tool", core_tools()).await;
    assert_eq!(
        result.stop_reason.as_deref(),
        Some("error_missing_tool_calls")
    );
    assert_eq!(count_persisted_nudges(&result), 0);
}
