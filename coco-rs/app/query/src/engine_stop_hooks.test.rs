//! Regression tests for the C3 death-spiral guard.
//!
//! Plan completion criterion (`docs/plan/.../summary-in-chinese-roi-cozy-lampson.md`):
//! "4 项 HIGH bug 修复并有 regression test：C3 + C15 + N1 + N2".

use std::sync::Arc;

use coco_hooks::HookRegistry;
use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason as LlmStopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::MessageHistory;
use coco_messages::create_user_message;
use coco_tool_runtime::ToolRegistry;
use coco_types::messages::ApiError;
use coco_types::messages::AssistantMessage;
use coco_types::messages::Message;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::StopHookDecision;
use super::last_assistant_api_error_payload;
use crate::config::ContinueReason;
use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopTurnState;

fn assistant_with_api_error(text: &str) -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: coco_llm_types::LlmMessage::assistant(vec![]),
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: Some(ApiError {
            message: text.to_string(),
            status_code: Some(400),
            error_type: Some("prompt_too_long".into()),
        }),
    }))
}

fn assistant_clean() -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: coco_llm_types::LlmMessage::assistant(vec![]),
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

fn user_message(text: &str) -> Arc<Message> {
    Arc::new(create_user_message(text))
}

fn history_from(messages: Vec<Arc<Message>>) -> MessageHistory {
    let mut h = MessageHistory::new();
    for m in messages {
        h.push_arc(m);
    }
    h
}

/// Minimal `LanguageModel` stub so the test can build a model runtime
/// without spinning up a real provider. `run_stop_hooks` never reaches
/// the model, so the methods just need to satisfy the trait.
struct StubModel;

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        "stub-model"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "stub".into(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(LlmStopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        Err(AISdkError::new("no stream"))
    }
}

fn engine_with_hooks(hooks: Option<Arc<HookRegistry>>) -> QueryEngine {
    let model: Arc<dyn LanguageModel> = Arc::new(StubModel);
    let client = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    QueryEngine::new(QueryEngineConfig::default(), client, tools, cancel, hooks)
}

fn loop_turn_state() -> LoopTurnState {
    LoopTurnState::new(
        /*max_tokens*/ None,
        /*max_turns*/ Some(100),
        /*max_continuations*/ 3,
    )
}

/// C3 finding: when the last assistant message carries an `api_error`
/// the death-spiral guard must surface BOTH the human-readable details
/// (forwarded to `executeStopFailureHooks` as `error_details`) and the
/// canonical short code (forwarded as `error`, the field hook matchers
/// filter on).
#[test]
fn c3_last_assistant_api_error_payload_returns_typed_payload_when_present() {
    let history = history_from(vec![
        user_message("hello"),
        assistant_with_api_error("rate limited; retry after 60s"),
    ]);

    let got = last_assistant_api_error_payload(&history).expect("payload must be Some");
    assert_eq!(
        got.message, "rate limited; retry after 60s",
        "api_error message text must be surfaced for the StopFailure hook payload",
    );
    assert_eq!(
        got.error_type.as_deref(),
        Some("prompt_too_long"),
        "error_type must round-trip from ApiError so hook matchers can filter by short code",
    );
}

/// C3 finding: a clean assistant message (no `api_error`) must NOT
/// trigger the death-spiral guard — otherwise normal Stop hooks would
/// never fire.
#[test]
fn c3_last_assistant_api_error_payload_is_none_when_clean() {
    let history = history_from(vec![user_message("hi"), assistant_clean()]);

    let got = last_assistant_api_error_payload(&history);
    assert!(
        got.is_none(),
        "clean assistant message must not short-circuit Stop hooks (got {got:?})",
    );
}

/// C3 finding: the guard must skip non-assistant trailers (tool
/// results, system messages, attachments) and walk back to the most
/// recent assistant message. Tool results are the most common case
/// because the loop runs PostToolUse before re-entering Stop logic.
#[test]
fn c3_last_assistant_api_error_payload_walks_past_user_trailer() {
    let history = history_from(vec![
        user_message("first prompt"),
        assistant_with_api_error("overloaded; provider returned 529"),
        user_message("retry"),
    ]);

    // Walking back past the trailing user message finds the assistant
    // api_error; this is the no-tool-calls terminal shape.
    let got = last_assistant_api_error_payload(&history).expect("payload must be Some");
    assert_eq!(
        got.message, "overloaded; provider returned 529",
        "guard must walk past user trailer to reach the last assistant message",
    );
}

/// C3 finding: when there's no assistant message at all (history
/// contains only the initial user prompt), the guard returns None so
/// normal Stop-hook flow runs.
#[test]
fn c3_last_assistant_api_error_payload_empty_history_is_none() {
    let history = history_from(vec![user_message("just submitted")]);
    assert!(last_assistant_api_error_payload(&history).is_none());
}

// ──────────────────────────────────────────────────────────────────────
// C3 — `run_stop_hooks` dispatcher integration
// ──────────────────────────────────────────────────────────────────────

/// C3 dispatcher integration: when the most recent assistant message
/// carries an `api_error`, `run_stop_hooks` MUST return
/// [`StopHookDecision::SkippedApiError`] WITHOUT invoking the configured
/// Stop hooks — even when a `HookRegistry` is wired into the engine.
/// This is the death-spiral guard's primary contract; without it a
/// Stop hook configured to block on terminal errors would re-block the
/// retry, which would re-emit the api_error, ad infinitum.
#[tokio::test]
async fn c3_run_stop_hooks_skips_when_last_assistant_is_api_error() {
    // Empty `HookRegistry` exercises the `Some(hooks)` branch of the
    // dispatcher (the C3 guard fires before hooks are consulted, so
    // registration content doesn't matter — only that `self.hooks` is
    // `Some`).
    let engine = engine_with_hooks(Some(Arc::new(HookRegistry::new())));
    let mut history = history_from(vec![
        user_message("prompt"),
        assistant_with_api_error("API Error: context window exceeded"),
    ]);
    let mut turn_state = loop_turn_state();

    let decision = engine
        .run_stop_hooks(
            &mut history,
            /*event_tx*/ &None,
            /*hook_tx_opt*/ None,
            &mut turn_state,
            /*response_text*/ "",
        )
        .await;

    match &decision {
        StopHookDecision::SkippedApiError { error_type } => {
            assert_eq!(
                error_type.as_deref(),
                Some("prompt_too_long"),
                "C3 must propagate the trailing api_error's error_type so the engine \
                 can use it as QueryResult.stop_reason (Finding R1)",
            );
        }
        other => panic!("api_error trailer must short-circuit to SkippedApiError, got {other:?}"),
    }
    assert!(
        turn_state.transition.is_none(),
        "C3 short-circuit must not set a transition — \
         the caller falls through to the no-tool-calls terminal",
    );
    assert!(
        !turn_state.stop_hook_active,
        "C3 short-circuit must not flip stop_hook_active — \
         that flag is reserved for the BlockedContinueLoop recursion path",
    );
}

/// C3 dispatcher integration: a clean assistant trailer (no api_error)
/// must NOT short-circuit. With `hooks: None` the dispatcher falls
/// through to the `Continue` decision so the no-tool-calls terminal
/// can finalize the turn normally.
#[tokio::test]
async fn c3_run_stop_hooks_continues_on_clean_assistant_when_no_hooks() {
    let engine = engine_with_hooks(None);
    let mut history = history_from(vec![user_message("prompt"), assistant_clean()]);
    let mut turn_state = loop_turn_state();

    let decision = engine
        .run_stop_hooks(
            &mut history,
            /*event_tx*/ &None,
            /*hook_tx_opt*/ None,
            &mut turn_state,
            /*response_text*/ "done",
        )
        .await;

    assert!(
        matches!(decision, StopHookDecision::Continue),
        "clean assistant + no hooks must yield Continue, got {decision:?}",
    );
    // Hooks weren't configured so the BlockedContinueLoop path can't
    // fire — flags remain at their initial state.
    assert!(turn_state.transition.is_none());
    assert!(!turn_state.stop_hook_active);
    // `Continue` is distinct from `BlockedContinueLoop`: the latter
    // mutates `transition` to `StopHookBlocking`; this path must not.
    assert!(
        !matches!(
            turn_state.transition,
            Some(ContinueReason::StopHookBlocking)
        ),
        "Continue path must not set StopHookBlocking",
    );
}
