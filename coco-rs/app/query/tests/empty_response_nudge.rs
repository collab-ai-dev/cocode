//! P0.3 — empty-response nudge/retry (hermes absorption).
//!
//! Empty and malformed tool-use terminals retry through a prompt-only
//! assistant/user overlay. Recovery scaffolding must never persist.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod mock_harness;

use std::sync::Arc;

use coco_inference::LanguageModel;
use coco_query::{QueryEngine, QueryEngineConfig, QueryOutcome, QueryResult};
use coco_types::PermissionMode;
use mock_harness::{MockModelBuilder, MockResponse, core_tools, run_with_mock};
use tokio_util::sync::CancellationToken;

const NUDGE_MARKER: &str = "empty response";

fn count_persisted_nudges(result: &QueryResult) -> usize {
    result
        .final_messages
        .iter()
        .filter(|message| stored_message_text(message.as_ref()).contains(NUDGE_MARKER))
        .count()
}

fn llm_role_and_text(message: &coco_llm_types::LlmMessage) -> (&'static str, String) {
    use coco_llm_types::AssistantContentPart;
    use coco_llm_types::LlmMessage;
    use coco_llm_types::UserContentPart;
    match message {
        LlmMessage::User { content, .. } => (
            "user",
            content
                .iter()
                .filter_map(|part| match part {
                    UserContentPart::Text(text) => Some(text.text.as_str()),
                    UserContentPart::File(_) => None,
                })
                .collect(),
        ),
        LlmMessage::Assistant { content, .. } => (
            "assistant",
            content
                .iter()
                .filter_map(|part| match part {
                    AssistantContentPart::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect(),
        ),
        LlmMessage::System { .. } => ("system", String::new()),
        LlmMessage::Developer { .. } => ("developer", String::new()),
        LlmMessage::Tool { .. } => ("tool", String::new()),
    }
}

fn stored_message_text(message: &coco_messages::Message) -> String {
    match message {
        coco_messages::Message::User(user) => llm_role_and_text(&user.message).1,
        coco_messages::Message::Assistant(assistant) => llm_role_and_text(&assistant.message).1,
        coco_messages::Message::Attachment(attachment) => attachment.as_text_for_display(),
        coco_messages::Message::System(_)
        | coco_messages::Message::ToolResult(_)
        | coco_messages::Message::Progress(_)
        | coco_messages::Message::Tombstone(_) => String::new(),
    }
}

fn assert_recovery_tail(prompt: &[coco_llm_types::LlmMessage], expected_attempts: usize) {
    let expected_messages = expected_attempts * 2;
    let tail = &prompt[prompt.len() - expected_messages..];
    for (attempt, pair) in tail.chunks_exact(2).enumerate() {
        let (assistant_role, _) = llm_role_and_text(&pair[0]);
        let (nudge_role, nudge) = llm_role_and_text(&pair[1]);
        assert_eq!(assistant_role, "assistant", "attempt {attempt}");
        assert_eq!(nudge_role, "user", "attempt {attempt}");
        assert!(
            nudge.contains("empty response"),
            "attempt {attempt}: {nudge}"
        );
    }
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

async fn run_with_mock_and_policy_events(
    model: Arc<dyn LanguageModel>,
    prompt: &str,
    policy: coco_config::EmptyResponsePolicy,
) -> (QueryResult, Vec<coco_types::CoreEvent>) {
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
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = event_rx.recv().await {
            events.push(event);
        }
        events
    });
    let result = engine
        .run_with_events(prompt, event_tx, coco_types::TurnId::from("cycle-1"))
        .await
        .expect("mock engine should not fail at the transport seam");
    let events = collector.await.expect("event collector");
    (result, events)
}

#[tokio::test]
async fn empty_then_text_gets_one_nudge_and_completes() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text(""))
        .on_call(1, |options| {
            assert_recovery_tail(&options.prompt, 1);
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
        .on_call(1, |options| {
            assert_recovery_tail(&options.prompt, 1);
            MockResponse::text("")
        })
        .on_call(2, |options| {
            assert_recovery_tail(&options.prompt, 2);
            MockResponse::text("")
        })
        .on_call(3, |options| {
            assert_recovery_tail(&options.prompt, 3);
            MockResponse::text("")
        })
        .on_call(4, |_| MockResponse::text("never reached"))
        .build();
    let (result, events) =
        run_with_mock_and_policy_events(model, "hello", coco_config::EmptyResponsePolicy::Nudge)
            .await;
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
            .any(|message| stored_message_text(message.as_ref()).contains("never reached")),
        "the loop must stop after the cap, not keep polling the model"
    );
    let QueryOutcome::Failed(failure) = &result.outcome else {
        panic!("recovery exhaustion must be typed as failure");
    };
    assert_eq!(failure.code, coco_types::ErrorCode::Provider);
    let ended: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(params)) => {
                Some(params)
            }
            _ => None,
        })
        .collect();
    assert_eq!(ended.len(), 1);
    let coco_types::TurnOutcome::Failed(ended_failure) = &ended[0].outcome else {
        panic!("TurnEnded must be failed");
    };
    assert_eq!(ended_failure.error.code, coco_types::ErrorCode::Provider);
    assert_eq!(ended[0].usage, Some(result.total_usage));
    assert!(
        ended[0]
            .session_result
            .as_ref()
            .is_some_and(|session| session.is_error && session.result.is_none())
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::MessageAppended {
            message,
            ..
        }) if stored_message_text(message.as_ref()).contains(NUDGE_MARKER)
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                coco_types::CoreEvent::Stream(
                    coco_types::AgentStreamEvent::ResponseAttemptDiscarded { .. }
                )
            ))
            .count(),
        4,
        "every malformed provider attempt must close with discard"
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        coco_types::CoreEvent::Stream(
            coco_types::AgentStreamEvent::ResponseAttemptCommitted { .. }
        )
    )));
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
            let text = llm_role_and_text(&u.message).1;
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

/// `Off` opts out of the whole mechanism, not just the empty-response half —
/// otherwise opting out still leaves the turn able to fail with a typed
/// provider error after three silent retries.
#[tokio::test]
async fn off_policy_also_disables_missing_tool_call_recovery() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::text_with_stop("I will run it.", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(1, |_| MockResponse::text("never reached"))
        .build();
    let result =
        run_with_mock_and_policy(model, "run it", coco_config::EmptyResponsePolicy::Off).await;
    assert_eq!(result.response_text, "I will run it.");
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
    assert!(matches!(result.outcome, QueryOutcome::Completed));
}

#[tokio::test]
async fn empty_after_tool_use_gets_post_tool_wording() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::tool_call("Bash", serde_json::json!({"command": "echo hi"}))
        })
        .on_call(1, |_| MockResponse::text(""))
        .on_call(2, |options| {
            let tail = &options.prompt[options.prompt.len() - 2..];
            let (assistant_role, _) = llm_role_and_text(&tail[0]);
            let (nudge_role, nudge) = llm_role_and_text(&tail[1]);
            assert_eq!(assistant_role, "assistant");
            assert_eq!(nudge_role, "user");
            assert!(nudge.contains("process the tool results above"));
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
            let tail = &options.prompt[options.prompt.len() - 2..];
            let (assistant_role, assistant) = llm_role_and_text(&tail[0]);
            let (nudge_role, nudge) = llm_role_and_text(&tail[1]);
            assert_eq!(assistant_role, "assistant");
            assert_eq!(assistant, "I'll inspect the file now.");
            assert_eq!(nudge_role, "user");
            assert!(nudge.contains("no tool call was provided"));
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
            .any(|message| stored_message_text(message.as_ref())
                .contains("I'll inspect the file now.")),
        "malformed assistant narration must remain transient"
    );
}

#[tokio::test]
async fn recovery_context_is_retired_once_a_response_commits() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::text_with_stop("I will run it.", coco_llm_types::StopReason::ToolUse)
        })
        .on_call(1, |options| {
            // The retry request carries the malformed attempt + its nudge, so
            // the model can see what it did wrong.
            let (assistant_role, malformed) =
                llm_role_and_text(&options.prompt[options.prompt.len() - 2]);
            let (nudge_role, nudge) = llm_role_and_text(&options.prompt[options.prompt.len() - 1]);
            assert_eq!(assistant_role, "assistant");
            assert_eq!(malformed, "I will run it.");
            assert_eq!(nudge_role, "user");
            assert!(nudge.contains("no tool call was provided"));
            MockResponse::tool_call("Bash", serde_json::json!({"command": "echo recovered"}))
        })
        .on_call(2, |options| {
            // That response committed, so the scaffolding is gone: the nudge is
            // phrased as a standing user instruction and must not linger in
            // every later request of the cycle.
            let roles: Vec<_> = options
                .prompt
                .iter()
                .map(|message| llm_role_and_text(message).0)
                .collect();
            assert_eq!(&roles[roles.len() - 2..], ["assistant", "tool"]);
            assert!(
                !options
                    .prompt
                    .iter()
                    .any(|message| llm_role_and_text(message)
                        .1
                        .contains("no tool call was provided")),
                "retired nudge must not reach a later request: {roles:?}"
            );
            MockResponse::text("Tool result processed")
        })
        .build();

    let result = run_with_mock(model, "run it", core_tools()).await;
    assert_eq!(result.response_text, "Tool result processed");
    assert_eq!(count_persisted_nudges(&result), 0);
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

    let (result, events) = run_with_mock_and_policy_events(
        model,
        "use a tool",
        coco_config::EmptyResponsePolicy::Nudge,
    )
    .await;
    assert_eq!(
        result.stop_reason.as_deref(),
        Some("error_missing_tool_calls")
    );
    assert_eq!(count_persisted_nudges(&result), 0);
    assert!(matches!(
        result.outcome,
        QueryOutcome::Failed(coco_types::ErrorPayload {
            code: coco_types::ErrorCode::Provider,
            ..
        })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(
                    coco_types::TurnEndedParams {
                        outcome: coco_types::TurnOutcome::Failed(_),
                        ..
                    }
                ))
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn abnormal_stop_is_a_typed_failure_not_a_completed_turn() {
    let model = MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::text_with_stop(
                "provider refusal",
                coco_llm_types::StopReason::ContentFilter,
            )
        })
        .build();

    let (result, events) =
        run_with_mock_and_policy_events(model, "respond", coco_config::EmptyResponsePolicy::Nudge)
            .await;
    let QueryOutcome::Failed(error) = &result.outcome else {
        panic!("content-filter terminal must fail the query");
    };
    assert_eq!(error.code, coco_types::ErrorCode::Provider);
    assert_eq!(result.stop_reason.as_deref(), Some("content_filter"));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(
                    coco_types::TurnEndedParams {
                        outcome: coco_types::TurnOutcome::Failed(_),
                        ..
                    }
                ))
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(
            coco_types::TurnEndedParams {
                outcome: coco_types::TurnOutcome::Completed(_),
                ..
            }
        ))
    )));
}
