//! Regression tests for the C3 death-spiral guard.
//!
//! Plan completion criterion (`docs/plan/.../summary-in-chinese-roi-cozy-lampson.md`):
//! "4 项 HIGH bug 修复并有 regression test：C3 + C15 + N1 + N2".

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_hooks::HookRegistry;
use coco_hooks::orchestration;
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
use coco_session::TranscriptIo;
use coco_tool_runtime::ToolRegistry;
use coco_types::ActiveGoal;
use coco_types::ToolAppState;
use coco_types::messages::ApiError;
use coco_types::messages::AssistantMessage;
use coco_types::messages::AttachmentBody;
use coco_types::messages::Message;
use coco_types::messages::SilentPayload;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::StopHookDecision;
use super::is_goal_prompt_hook;
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
    QueryEngine::new(
        QueryEngineConfig::default(),
        coco_types::SessionId::try_new("test-session").unwrap(),
        client,
        tools,
        cancel,
        hooks,
    )
}

struct StaticTaskHandle {
    tasks: Vec<coco_types::TaskStateBase>,
}

#[async_trait::async_trait]
impl coco_tool_runtime::TaskHandle for StaticTaskHandle {
    async fn list_tasks(&self) -> Vec<coco_types::TaskStateBase> {
        self.tasks.clone()
    }
}

fn running_task() -> coco_types::TaskStateBase {
    coco_types::TaskStateBase {
        id: "task-1".to_string(),
        status: coco_types::TaskStatus::Running,
        notified: false,
        description: "background check".to_string(),
        tool_use_id: None,
        start_time: 0,
        end_time: None,
        killed_by: None,
        total_paused_ms: None,
        output_file: None,
        output_offset: 0,
        extras: coco_types::TaskExtras::shell_default(),
    }
}

fn goal_hook(condition: &str) -> coco_hooks::HookDefinition {
    coco_hooks::HookDefinition {
        event: coco_types::HookEventType::Stop,
        matcher: None,
        handler: coco_hooks::HookHandler::Prompt {
            prompt: condition.to_string(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: coco_types::HookScope::Session,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
        managed_by: Some(coco_hooks::ManagedHookKind::Goal),
    }
}

fn active_goal(condition: &str) -> ActiveGoal {
    ActiveGoal {
        condition: condition.to_string(),
        iterations: 2,
        set_at_ms: 1,
        tokens_at_start: 0,
        last_reason: Some("previously blocked".to_string()),
    }
}

fn goal_statuses(history: &MessageHistory) -> Vec<coco_types::GoalStatusPayload> {
    history
        .as_slice()
        .iter()
        .filter_map(|message| {
            let Message::Attachment(attachment) = message.as_ref() else {
                return None;
            };
            let AttachmentBody::Silent(SilentPayload::GoalStatus(payload)) = &attachment.body
            else {
                return None;
            };
            Some(payload.clone())
        })
        .collect()
}

fn loop_turn_state() -> LoopTurnState {
    LoopTurnState::new(
        /*max_tokens*/ None,
        /*max_turns*/ Some(100),
        /*max_continuations*/ 3,
    )
}

#[test]
fn goal_prompt_hook_matcher_requires_managed_session_stop_prompt() {
    let hook = goal_hook("finish migration");
    assert!(is_goal_prompt_hook(&hook, "finish migration"));
    assert!(!is_goal_prompt_hook(&hook, "other goal"));

    let mut unmanaged = hook.clone();
    unmanaged.managed_by = None;
    assert!(!is_goal_prompt_hook(&unmanaged, "finish migration"));

    let mut matched = hook;
    matched.matcher = Some("*".to_string());
    assert!(!is_goal_prompt_hook(&matched, "finish migration"));
}

#[tokio::test]
async fn goal_terminal_success_clears_active_goal_hook_and_records_status() {
    let hooks = Arc::new(HookRegistry::new());
    hooks.register(goal_hook("finish migration"));
    let mut engine = engine_with_hooks(Some(hooks.clone()));
    let store = Arc::new(coco_session::InMemoryStore::new());
    engine.transcript_store = Some(store.clone());
    engine.transcript_session_id =
        Some(coco_types::SessionId::try_new("goal-success-session").unwrap());
    let terminal_goal_metadata_written = Arc::new(AtomicBool::new(false));
    engine.terminal_goal_metadata_written = Some(terminal_goal_metadata_written.clone());
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_goal: Some(active_goal("finish migration")),
        ..ToolAppState::default()
    }));
    engine.app_state = Some(app_state.clone());
    let mut history = MessageHistory::new();

    engine
        .handle_active_goal_terminal_result(
            &orchestration::AggregatedHookResult {
                llm_successes: vec![orchestration::LlmHookSuccess {
                    source: orchestration::HookBlockingSource::Prompt {
                        prompt: "finish migration".to_string(),
                    },
                    reason: Some("all checks passed".to_string()),
                }],
                ..Default::default()
            },
            &mut history,
            &None,
        )
        .await;

    assert!(app_state.read().await.active_goal.is_none());
    assert!(
        hooks
            .find_matching(coco_types::HookEventType::Stop, None)
            .is_empty(),
        "terminal success must remove the managed goal hook"
    );
    let statuses = goal_statuses(&history);
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0].met);
    assert!(!statuses[0].failed);
    assert!(!statuses[0].sentinel);
    assert_eq!(statuses[0].condition, "finish migration");
    assert_eq!(statuses[0].reason.as_deref(), Some("all checks passed"));
    assert_eq!(statuses[0].iterations, Some(3));
    let metadata = store.read_metadata("goal-success-session").unwrap();
    let goal = metadata
        .goal
        .expect("success stores terminal goal metadata");
    assert!(goal.met);
    assert_eq!(goal.condition, "finish migration");
    assert_eq!(goal.iterations, 3);
    assert_eq!(goal.last_reason, None);
    assert!(terminal_goal_metadata_written.load(Ordering::SeqCst));

    engine.persist_goal_metadata(None).await;
    assert!(!terminal_goal_metadata_written.load(Ordering::SeqCst));
}

#[tokio::test]
async fn goal_terminal_impossible_clears_goal_and_records_failed_status() {
    let hooks = Arc::new(HookRegistry::new());
    hooks.register(goal_hook("finish migration"));
    let mut engine = engine_with_hooks(Some(hooks.clone()));
    let store = Arc::new(coco_session::InMemoryStore::new());
    engine.transcript_store = Some(store.clone());
    engine.transcript_session_id =
        Some(coco_types::SessionId::try_new("goal-impossible-session").unwrap());
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_goal: Some(active_goal("finish migration")),
        ..ToolAppState::default()
    }));
    engine.app_state = Some(app_state.clone());
    let mut history = MessageHistory::new();

    engine
        .handle_active_goal_terminal_result(
            &orchestration::AggregatedHookResult {
                llm_impossibles: vec![orchestration::LlmHookImpossible {
                    source: orchestration::HookBlockingSource::Prompt {
                        prompt: "finish migration".to_string(),
                    },
                    reason: "remote branch was deleted".to_string(),
                }],
                ..Default::default()
            },
            &mut history,
            &None,
        )
        .await;

    assert!(app_state.read().await.active_goal.is_none());
    assert!(
        hooks
            .find_matching(coco_types::HookEventType::Stop, None)
            .is_empty(),
        "terminal impossible verdict must remove the managed goal hook"
    );
    let statuses = goal_statuses(&history);
    assert_eq!(statuses.len(), 1);
    assert!(!statuses[0].met);
    assert!(statuses[0].failed);
    assert!(!statuses[0].sentinel);
    assert_eq!(statuses[0].condition, "finish migration");
    assert_eq!(
        statuses[0].reason.as_deref(),
        Some("remote branch was deleted")
    );
    assert_eq!(statuses[0].iterations, Some(3));
    assert_eq!(
        store.read_metadata("goal-impossible-session").unwrap().goal,
        None
    );
}

#[tokio::test]
async fn goal_blocked_updates_active_goal_and_records_unmet_status() {
    let mut engine = engine_with_hooks(Some(Arc::new(HookRegistry::new())));
    let store = Arc::new(coco_session::InMemoryStore::new());
    engine.transcript_store = Some(store.clone());
    engine.transcript_session_id =
        Some(coco_types::SessionId::try_new("goal-blocked-session").unwrap());
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_goal: Some(ActiveGoal {
            condition: "finish migration".to_string(),
            iterations: 0,
            set_at_ms: 1,
            tokens_at_start: 0,
            last_reason: None,
        }),
        ..ToolAppState::default()
    }));
    engine.app_state = Some(app_state.clone());
    let mut history = MessageHistory::new();

    engine
        .record_active_goal_blocked(
            &mut history,
            &None,
            &orchestration::HookBlockingError {
                blocking_error: "tests are still failing".to_string(),
                source: orchestration::HookBlockingSource::Prompt {
                    prompt: "finish migration".to_string(),
                },
            },
        )
        .await;

    let active = app_state
        .read()
        .await
        .active_goal
        .clone()
        .expect("goal stays active");
    assert_eq!(active.iterations, 1);
    assert_eq!(
        active.last_reason.as_deref(),
        Some("tests are still failing")
    );
    let statuses = goal_statuses(&history);
    assert_eq!(statuses.len(), 1);
    assert!(!statuses[0].met);
    assert!(!statuses[0].failed);
    assert!(!statuses[0].sentinel);
    assert_eq!(statuses[0].condition, "finish migration");
    assert_eq!(
        statuses[0].reason.as_deref(),
        Some("tests are still failing")
    );
    let metadata = store.read_metadata("goal-blocked-session").unwrap();
    let goal = metadata.goal.expect("blocked stores active goal metadata");
    assert!(!goal.met);
    assert_eq!(goal.condition, "finish migration");
    assert_eq!(goal.iterations, 1);
    assert_eq!(goal.last_reason.as_deref(), Some("tests are still failing"));
}

#[tokio::test]
async fn goal_stop_hook_evaluation_is_deferred_while_background_tasks_run() {
    let hooks = Arc::new(HookRegistry::new());
    hooks.register(goal_hook("finish migration"));
    let mut engine = engine_with_hooks(Some(hooks.clone()));
    engine.task_handle = Some(Arc::new(StaticTaskHandle {
        tasks: vec![running_task()],
    }));
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_goal: Some(active_goal("finish migration")),
        ..ToolAppState::default()
    }));
    engine.app_state = Some(app_state.clone());
    let mut history = history_from(vec![user_message("prompt"), assistant_clean()]);
    let mut turn_state = loop_turn_state();

    let decision = engine
        .run_stop_hooks(
            &mut history,
            /*event_tx*/ &None,
            /*hook_tx_opt*/ None,
            &mut turn_state,
            /*response_text*/ "done for now",
        )
        .await;

    assert!(matches!(decision, StopHookDecision::Continue));
    let active = app_state
        .read()
        .await
        .active_goal
        .clone()
        .expect("goal stays active while background task runs");
    assert_eq!(active.iterations, 2);
    assert_eq!(active.last_reason.as_deref(), Some("previously blocked"));
    assert!(
        !hooks
            .find_matching(coco_types::HookEventType::Stop, None)
            .is_empty(),
        "deferred goal hook must be restored after this Stop pass"
    );
    assert!(
        goal_statuses(&history).is_empty(),
        "deferred goal evaluation must not record unmet/met status"
    );
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
