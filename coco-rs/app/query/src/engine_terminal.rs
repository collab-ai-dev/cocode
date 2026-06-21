//! Terminal branch helpers for `QueryEngine::run_session_loop_inner`.

use coco_llm_types::ToolCallPart;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_types::TokenUsage;
use tracing::info;
use tracing::warn;

use crate::ContinueReason;
use crate::QueryResult;
use crate::emit::emit_protocol;
use crate::emit::emit_stream;
use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopTurnState;
use crate::engine_result::make_query_result;
use crate::helpers::budget_pct_used;
use crate::helpers::should_continue_for_budget;

pub(crate) enum NoToolCallsTerminal {
    ContinueLoop,
    Return(Box<QueryResult>),
}

#[allow(clippy::too_many_arguments)]
impl QueryEngine {
    pub(crate) async fn handle_blocking_limit_terminal(
        &self,
        consts: &LoopConstants,
        acc: &LoopAccumulator,
        turn_state: &LoopTurnState,
        active_snapshot: &coco_inference::ModelRuntimeSnapshot,
        estimated_tokens: i64,
        context_window: i64,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        cycle_turn_id: Option<coco_types::TurnId>,
        total_usage: &TokenUsage,
    ) -> QueryResult {
        warn!(
            estimated_tokens,
            context_window,
            provider = active_snapshot.provider,
            model_id = active_snapshot.model_id,
            "pre-API blocking limit hit — estimated prompt exceeds model context",
        );
        crate::history_sync::history_push_and_emit(
            history,
            crate::helpers::build_blocking_limit_api_error_message(
                estimated_tokens,
                context_window,
            ),
            event_tx,
        )
        .await;
        if let Some(id) = cycle_turn_id.as_ref() {
            let _ = emit_protocol(
                event_tx,
                crate::ServerNotification::TurnEnded(coco_types::TurnEndedParams::failed(
                    id.clone(),
                    Some(*total_usage),
                    coco_types::ErrorPayload {
                        message: format!(
                            "blocking_limit: estimated {estimated_tokens} tokens \
                                 exceeds active model context window {context_window} \
                                 (provider={}, model={})",
                            active_snapshot.provider, active_snapshot.model_id,
                        ),
                        code: coco_types::ErrorCode::Provider,
                    },
                )),
            )
            .await;
        }
        make_query_result(
            consts,
            acc,
            turn_state,
            String::new(),
            /*cancelled*/ false,
            /*budget_exhausted*/ false,
            Some("blocking_limit".into()),
            history.to_vec(),
            history.snapshot(),
        )
    }

    /// Terminal handler for an oversized image at the API boundary.
    /// Push a synthetic api_error assistant message, emit `TurnEnded(failed)`,
    /// and end the turn without sending the request — the image can't be
    /// auto-shrunk here, so the user must resize and retry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_image_too_large_terminal(
        &self,
        consts: &LoopConstants,
        acc: &LoopAccumulator,
        turn_state: &LoopTurnState,
        err: &coco_messages::ImageSizeError,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        cycle_turn_id: Option<coco_types::TurnId>,
        total_usage: &TokenUsage,
    ) -> QueryResult {
        let message = err.message();
        warn!(
            base64_len = err.base64_len,
            limit = err.max_base64_len,
            "oversized image at API boundary — request not sent",
        );
        crate::history_sync::history_push_and_emit(
            history,
            coco_messages::create_assistant_error_message(&message, None, Some("invalid_request")),
            event_tx,
        )
        .await;
        if let Some(id) = cycle_turn_id.as_ref() {
            let _ = emit_protocol(
                event_tx,
                crate::ServerNotification::TurnEnded(coco_types::TurnEndedParams::failed(
                    id.clone(),
                    Some(*total_usage),
                    coco_types::ErrorPayload {
                        message: message.clone(),
                        code: coco_types::ErrorCode::Provider,
                    },
                )),
            )
            .await;
        }
        make_query_result(
            consts,
            acc,
            turn_state,
            String::new(),
            /*cancelled*/ false,
            /*budget_exhausted*/ false,
            Some("image_too_large".into()),
            history.to_vec(),
            history.snapshot(),
        )
    }

    pub(crate) async fn handle_usd_budget_terminal(
        &self,
        consts: &LoopConstants,
        acc: &LoopAccumulator,
        turn_state: &LoopTurnState,
        response_text: String,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        cycle_turn_id: Option<coco_types::TurnId>,
        total_usage: &TokenUsage,
        tool_calls: &[ToolCallPart],
        total_cost_usd: f64,
        max_budget_usd: f64,
    ) -> QueryResult {
        append_budget_skipped_tool_results(history, event_tx, tool_calls, &self.tools).await;
        if let Some(id) = cycle_turn_id.as_ref() {
            let _ = emit_protocol(
                event_tx,
                crate::ServerNotification::TurnEnded(coco_types::TurnEndedParams::failed(
                    id.clone(),
                    Some(*total_usage),
                    coco_types::ErrorPayload {
                        message: format!(
                            "maximum USD budget reached (${total_cost_usd:.4} / ${max_budget_usd:.4})"
                        ),
                        code: coco_types::ErrorCode::Resource,
                    },
                )),
            )
            .await;
        }
        make_query_result(
            consts,
            acc,
            turn_state,
            response_text,
            /*cancelled*/ false,
            /*budget_exhausted*/ true,
            Some("error_max_budget_usd".into()),
            history.to_vec(),
            history.snapshot(),
        )
    }

    pub(crate) async fn handle_no_tool_calls_terminal(
        &self,
        consts: &LoopConstants,
        acc: &mut LoopAccumulator,
        turn_state: &mut LoopTurnState,
        response_text: String,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        hook_tx_opt: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        cycle_turn_id: Option<coco_types::TurnId>,
        usage: TokenUsage,
        parsed_stop_reason: Option<coco_messages::StopReason>,
    ) -> NoToolCallsTerminal {
        if self.tools.get_by_name("StructuredOutput").is_some()
            && acc.run_artifacts.structured_output.is_none()
        {
            let max_retries = crate::config::max_structured_output_retries();
            if acc.run_artifacts.structured_output_attempts >= max_retries {
                warn!(
                    attempts = acc.run_artifacts.structured_output_attempts,
                    max_retries, "structured output retry cap exceeded"
                );
                self.emit_successful_turn_completed(
                    event_tx,
                    history,
                    usage,
                    cycle_turn_id.clone(),
                    parsed_stop_reason,
                )
                .await;
                return NoToolCallsTerminal::Return(Box::new(make_query_result(
                    consts,
                    &*acc,
                    &*turn_state,
                    response_text,
                    /*cancelled*/ false,
                    /*budget_exhausted*/ false,
                    Some("error_max_structured_output_retries".into()),
                    history.to_vec(),
                    history.snapshot(),
                )));
            }
        }

        self.flush_successful_turn_state(&mut *history).await;
        self.maybe_spawn_prompt_suggestion_after_stop(event_tx)
            .await;

        let stop_decision = self
            .run_stop_hooks(
                &mut *history,
                event_tx,
                hook_tx_opt,
                turn_state,
                &response_text,
            )
            .await;
        match stop_decision {
            crate::engine_stop_hooks::StopHookDecision::Prevented => {
                self.emit_successful_turn_completed(
                    event_tx,
                    history,
                    usage,
                    cycle_turn_id.clone(),
                    parsed_stop_reason,
                )
                .await;
                NoToolCallsTerminal::Return(Box::new(make_query_result(
                    consts,
                    &*acc,
                    &*turn_state,
                    response_text,
                    /*cancelled*/ false,
                    /*budget_exhausted*/ false,
                    Some("stop_hook_prevented".into()),
                    history.to_vec(),
                    history.snapshot(),
                )))
            }
            crate::engine_stop_hooks::StopHookDecision::BlockedContinueLoop => {
                NoToolCallsTerminal::ContinueLoop
            }
            crate::engine_stop_hooks::StopHookDecision::SkippedApiError { error_type } => {
                let stop_reason = error_type.unwrap_or_else(|| "end_turn_api_error".into());
                info!(
                    turn = turn_state.turn,
                    stop_reason = %stop_reason,
                    "ending turn early — last message is api_error (C3 guard)"
                );
                self.emit_successful_turn_completed(
                    event_tx,
                    history,
                    usage,
                    cycle_turn_id.clone(),
                    parsed_stop_reason,
                )
                .await;
                NoToolCallsTerminal::Return(Box::new(make_query_result(
                    consts,
                    &*acc,
                    &*turn_state,
                    response_text,
                    /*cancelled*/ false,
                    /*budget_exhausted*/ false,
                    Some(stop_reason),
                    history.to_vec(),
                    history.snapshot(),
                )))
            }
            crate::engine_stop_hooks::StopHookDecision::Continue => {
                if self.config.enable_token_budget_continuation
                    && should_continue_for_budget(&turn_state.budget)
                {
                    let pct = budget_pct_used(&turn_state.budget);
                    let nudge = format!(
                        "Token budget continuation: you've used {pct}% of the turn budget. \
                         Keep going — don't summarize or recap, just continue the work."
                    );
                    crate::history_sync::history_push_and_emit(
                        history,
                        coco_messages::create_meta_message(&nudge),
                        event_tx,
                    )
                    .await;
                    turn_state.budget.record_continuation();
                    turn_state.transition = Some(ContinueReason::TokenBudgetContinuation);
                    turn_state.stop_hook_active = false;
                    turn_state.max_tokens_recovery_count = 0;
                    {
                        let mut state = self.reactive_state.lock().await;
                        state.reset();
                    }
                    info!(turn = turn_state.turn, pct, "token budget continuation");
                    NoToolCallsTerminal::ContinueLoop
                } else {
                    info!(
                        turn = turn_state.turn,
                        response_chars = response_text.len(),
                        tokens_in = usage.input_tokens.total,
                        tokens_out = usage.output_tokens.total,
                        "no tool calls, conversation complete"
                    );
                    self.emit_successful_turn_completed(
                        event_tx,
                        history,
                        usage,
                        cycle_turn_id.clone(),
                        parsed_stop_reason,
                    )
                    .await;
                    NoToolCallsTerminal::Return(Box::new(make_query_result(
                        consts,
                        &*acc,
                        &*turn_state,
                        response_text,
                        /*cancelled*/ false,
                        /*budget_exhausted*/ false,
                        Some("end_turn".into()),
                        history.to_vec(),
                        history.snapshot(),
                    )))
                }
            }
        }
    }
}

async fn append_budget_skipped_tool_results(
    history: &mut MessageHistory,
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    tool_calls: &[ToolCallPart],
    tools: &coco_tool_runtime::ToolRegistry,
) {
    const BUDGET_SKIP_MESSAGE: &str =
        "Tool execution skipped because maximum USD budget was reached.";
    for tool_call in tool_calls {
        if history_contains_tool_result(history, &tool_call.tool_call_id) {
            continue;
        }
        let tool_id = tool_call
            .tool_name
            .parse()
            .unwrap_or_else(|_| coco_types::ToolId::Custom(tool_call.tool_name.clone()));
        let canonical_tool_id = tools.get(&tool_id).map(|tool| tool.id()).unwrap_or(tool_id);
        crate::history_sync::history_push_and_emit(
            history,
            coco_messages::create_error_tool_result(
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                canonical_tool_id,
                BUDGET_SKIP_MESSAGE,
            ),
            event_tx,
        )
        .await;
        let _ = emit_stream(
            event_tx,
            crate::AgentStreamEvent::ToolUseCompleted {
                call_id: tool_call.tool_call_id.clone(),
                name: tool_call.tool_name.clone(),
                output: BUDGET_SKIP_MESSAGE.to_string(),
                is_error: true,
            },
        )
        .await;
    }
}

fn history_contains_tool_result(history: &MessageHistory, tool_call_id: &str) -> bool {
    history.iter().any(|message| {
        matches!(
            message.as_ref(),
            Message::ToolResult(result) if result.tool_use_id == tool_call_id
        )
    })
}
