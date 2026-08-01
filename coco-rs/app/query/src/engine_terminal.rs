//! Terminal branch helpers for `QueryEngine::run_session_loop_inner`.

use coco_context::ContextualUserFragment as _;
use coco_llm_types::ToolCallPart;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_types::TokenUsage;
use tracing::info;
use tracing::warn;

use crate::ContinueReason;
use crate::QueryResult;
use crate::emit::emit_stream;
use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopTurnState;
use crate::engine_result::make_query_result;
use crate::engine_result::mark_query_failed;
use crate::helpers::budget_pct_used;
use crate::helpers::should_continue_for_budget;

pub(crate) enum NoToolCallsTerminal {
    ContinueLoop,
    Return {
        result: Box<QueryResult>,
        terminal: Option<coco_types::TurnEndedParams>,
    },
}

/// Max retries for a malformed or empty terminal response per user cycle.
pub(crate) const MAX_TERMINAL_RECOVERY_ATTEMPTS: i32 = 3;

/// Nudge wording when this cycle just executed tools (hermes #9400 case).
const EMPTY_RESPONSE_NUDGE_AFTER_TOOLS: &str = "You just executed tool calls but returned an empty response. Please process the tool results above and continue with the task.";

/// Generic nudge wording for an empty first response.
const EMPTY_RESPONSE_NUDGE_GENERIC: &str =
    "You returned an empty response. Please respond to the request above.";

const MISSING_TOOL_CALL_NUDGE: &str = "You indicated that you would use tools, but no tool call was provided. Emit the intended tool call now, or answer normally if no tool is needed.";
const EMPTY_RESPONSE_FAILURE: &str =
    "model returned an empty response after exhausting 3 recovery attempts";
const MISSING_TOOL_CALL_FAILURE: &str =
    "provider declared tool use without emitting a tool call after exhausting 3 recovery attempts";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalAnomaly {
    EmptyCleanResponse,
    ToolUseWithoutCalls,
}

pub(crate) fn classify_terminal_anomaly(
    response_text: &str,
    stop_reason: Option<coco_messages::StopReason>,
    empty_response_policy: coco_config::EmptyResponsePolicy,
    allow_empty_response_recovery: bool,
) -> Option<TerminalAnomaly> {
    if stop_reason == Some(coco_messages::StopReason::ToolUse) {
        return Some(TerminalAnomaly::ToolUseWithoutCalls);
    }
    if allow_empty_response_recovery
        && response_text.trim().is_empty()
        && empty_response_policy == coco_config::EmptyResponsePolicy::Nudge
        && matches!(
            stop_reason,
            None | Some(coco_messages::StopReason::EndTurn)
                | Some(coco_messages::StopReason::StopSequence)
        )
    {
        return Some(TerminalAnomaly::EmptyCleanResponse);
    }
    None
}

/// Whether any of the last few history messages is a tool result — picks
/// the post-tool nudge wording over the generic one.
fn recent_history_has_tool_result(history: &MessageHistory) -> bool {
    history
        .iter()
        .rev()
        .take(5)
        .any(|m| matches!(m.as_ref(), Message::ToolResult(_)))
}

#[allow(clippy::too_many_arguments)]
impl QueryEngine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_terminal_anomaly(
        &self,
        anomaly: TerminalAnomaly,
        assistant_msg: Message,
        consts: &LoopConstants,
        acc: &LoopAccumulator,
        turn_state: &mut LoopTurnState,
        history: &MessageHistory,
    ) -> NoToolCallsTerminal {
        let (attempt, nudge, transition, failure_reason, failure_message) = match anomaly {
            TerminalAnomaly::EmptyCleanResponse => {
                turn_state.empty_response_retries += 1;
                let attempt = turn_state.empty_response_retries;
                let nudge = if recent_history_has_tool_result(history) {
                    EMPTY_RESPONSE_NUDGE_AFTER_TOOLS
                } else {
                    EMPTY_RESPONSE_NUDGE_GENERIC
                };
                (
                    attempt,
                    nudge,
                    ContinueReason::EmptyResponseRecovery { attempt },
                    "error_empty_response_retries",
                    EMPTY_RESPONSE_FAILURE,
                )
            }
            TerminalAnomaly::ToolUseWithoutCalls => {
                turn_state.missing_tool_call_retries += 1;
                let attempt = turn_state.missing_tool_call_retries;
                (
                    attempt,
                    MISSING_TOOL_CALL_NUDGE,
                    ContinueReason::MissingToolCallRecovery { attempt },
                    "error_missing_tool_calls",
                    MISSING_TOOL_CALL_FAILURE,
                )
            }
        };

        if attempt <= MAX_TERMINAL_RECOVERY_ATTEMPTS {
            warn!(
                turn = turn_state.turn,
                attempt,
                ?anomaly,
                "retrying malformed terminal response"
            );
            let fragment = coco_context::TerminalRecoveryNudgeFragment::new(nudge);
            turn_state.recovery_context.append(
                history,
                assistant_msg,
                coco_messages::create_meta_message(&fragment.render()),
            );
            turn_state.transition = Some(transition);
            turn_state.stop_hook_active = false;
            turn_state.max_tokens_recovery_count = 0;
            return NoToolCallsTerminal::ContinueLoop;
        }

        turn_state.recovery_context.clear();
        warn!(turn = turn_state.turn, ?anomaly, %failure_message);
        let error = coco_types::ErrorPayload {
            message: failure_message.to_string(),
            code: coco_types::ErrorCode::Provider,
        };
        NoToolCallsTerminal::Return {
            result: Box::new(mark_query_failed(
                make_query_result(
                    consts,
                    acc,
                    turn_state,
                    String::new(),
                    /*cancelled*/ false,
                    /*budget_exhausted*/ false,
                    Some(failure_reason.into()),
                    history.to_vec(),
                    history.snapshot(),
                ),
                error,
            )),
            terminal: None,
        }
    }

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
        let error = coco_types::ErrorPayload {
            message: format!(
                "blocking_limit: estimated {estimated_tokens} tokens \
                 exceeds active model context window {context_window} \
                 (provider={}, model={})",
                active_snapshot.provider, active_snapshot.model_id,
            ),
            code: coco_types::ErrorCode::Provider,
        };
        mark_query_failed(
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
            ),
            error,
        )
    }

    /// Terminal handler for an oversized image at the API boundary.
    /// Push a synthetic api_error assistant message and return a typed failure;
    /// the session lifecycle owns `TurnEnded(Failed)`. The image cannot be
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
        let error = coco_types::ErrorPayload {
            message: message.clone(),
            code: coco_types::ErrorCode::Input,
        };
        mark_query_failed(
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
            ),
            error,
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
        tool_calls: &[ToolCallPart],
        total_cost_usd: f64,
        max_budget_usd: f64,
    ) -> QueryResult {
        append_budget_skipped_tool_results(history, event_tx, tool_calls, &self.tools).await;
        let error = coco_types::ErrorPayload {
            message: format!(
                "maximum USD budget reached (${total_cost_usd:.4} / ${max_budget_usd:.4})"
            ),
            code: coco_types::ErrorCode::Resource,
        };
        mark_query_failed(
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
            ),
            error,
        )
    }

    pub(crate) fn structured_output_retry_cap_error(&self) -> coco_types::ErrorPayload {
        let cap = self.config.max_structured_output_retries;
        coco_types::ErrorPayload {
            message: format!("Failed to provide valid structured output after {cap} attempts"),
            code: coco_types::ErrorCode::Provider,
        }
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
        if self.config.requires_structured_output
            && self.tools.get_by_name("StructuredOutput").is_some()
            && acc.run_artifacts.structured_output.is_none()
        {
            acc.run_artifacts.structured_output_failed_attempts = acc
                .run_artifacts
                .structured_output_failed_attempts
                .saturating_add(1);
            if crate::engine_tool_execution::structured_output_failure_limit_reached(
                &acc.run_artifacts,
                self.config.max_structured_output_retries,
            ) {
                return NoToolCallsTerminal::Return {
                    result: Box::new(mark_query_failed(
                        make_query_result(
                            consts,
                            acc,
                            turn_state,
                            response_text,
                            /*cancelled*/ false,
                            /*budget_exhausted*/ false,
                            Some("error_max_structured_output_retries".into()),
                            history.to_vec(),
                            history.snapshot(),
                        ),
                        self.structured_output_retry_cap_error(),
                    )),
                    terminal: None,
                };
            }
            turn_state.transition = Some(crate::ContinueReason::NextTurn);
            turn_state.stop_hook_active = false;
            turn_state.max_tokens_recovery_count = 0;
            return NoToolCallsTerminal::ContinueLoop;
        }

        self.flush_successful_turn_state(&mut *history).await;

        // Goal runtime (§10.2, §10.3): at a user-started goal turn's natural stop,
        // run the completion coordinator + `FinishTurn` and record the transition
        // cell. The engine executes exactly one logical turn and never
        // self-continues — the `GoalSupervisor` driver owns the next turn (it is
        // nudged once this turn's slot frees). Skipped when the supervisor already
        // owns this turn's finalization, so exactly one component finalizes.
        if let Some(goal_handle) = &self.goal_handle
            && goal_handle.is_available()
            && !self.goal_supervisor_owns_finalize
        {
            let finalization = goal_handle
                .finalize_goal_turn(
                    usage.input_tokens.total.max(0) as u64,
                    usage.output_tokens.total.max(0) as u64,
                    /*signals_present*/ true,
                )
                .await;
            self.emit_active_goal_changed(goal_handle.as_ref(), event_tx)
                .await;
            // One concise transcript cell per durable goal transition (§9.2), so
            // the conversation carries a permanent record of a completed / blocked
            // / paused / budget / usage / waiting transition.
            if let Some(payload) = finalization.transition {
                crate::history_sync::history_push_and_emit(
                    history,
                    coco_messages::Message::Attachment(
                        coco_messages::AttachmentMessage::silent_goal_status(payload),
                    ),
                    event_tx,
                )
                .await;
            }
        }

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

        // Text-only turn (no tool calls): fire the skill-learning review here
        // too, so this tail is covered symmetrically with the tool path. Inert
        // unless `Feature::SkillLearning` is on and the runtime was built.
        // Gated AFTER the stop hooks and skipped on `BlockedContinueLoop`: a
        // blocking Stop hook keeps the SAME user cycle running, so firing here
        // would tick the throttle twice for one delivered cycle and could
        // snapshot a history the hook is about to extend.
        if !matches!(
            stop_decision,
            crate::engine_stop_hooks::StopHookDecision::BlockedContinueLoop
        ) {
            self.run_skill_review_finalize(history, event_tx).await;
        }

        match stop_decision {
            crate::engine_stop_hooks::StopHookDecision::Prevented => {
                let terminal = self
                    .finish_successful_turn_completed(
                        event_tx,
                        history,
                        usage,
                        cycle_turn_id.clone(),
                        parsed_stop_reason,
                    )
                    .await;
                NoToolCallsTerminal::Return {
                    result: Box::new(make_query_result(
                        consts,
                        &*acc,
                        &*turn_state,
                        response_text,
                        /*cancelled*/ false,
                        /*budget_exhausted*/ false,
                        Some("stop_hook_prevented".into()),
                        history.to_vec(),
                        history.snapshot(),
                    )),
                    terminal,
                }
            }
            crate::engine_stop_hooks::StopHookDecision::BlockedContinueLoop => {
                NoToolCallsTerminal::ContinueLoop
            }
            crate::engine_stop_hooks::StopHookDecision::SkippedApiError { payload } => {
                let stop_reason = payload
                    .error_type
                    .clone()
                    .unwrap_or_else(|| "end_turn_api_error".into());
                info!(
                    turn = turn_state.turn,
                    stop_reason = %stop_reason,
                    "ending turn with typed failure — last message is api_error (C3 guard)"
                );
                let error = coco_types::ErrorPayload {
                    message: payload.message,
                    code: coco_types::ErrorCode::Provider,
                };
                NoToolCallsTerminal::Return {
                    result: Box::new(mark_query_failed(
                        make_query_result(
                            consts,
                            &*acc,
                            &*turn_state,
                            response_text,
                            /*cancelled*/ false,
                            /*budget_exhausted*/ false,
                            Some(stop_reason),
                            history.to_vec(),
                            history.snapshot(),
                        ),
                        error,
                    )),
                    terminal: None,
                }
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
                    let terminal = self
                        .finish_successful_turn_completed(
                            event_tx,
                            history,
                            usage,
                            cycle_turn_id.clone(),
                            parsed_stop_reason,
                        )
                        .await;
                    NoToolCallsTerminal::Return {
                        result: Box::new(make_query_result(
                            consts,
                            &*acc,
                            &*turn_state,
                            response_text,
                            /*cancelled*/ false,
                            /*budget_exhausted*/ false,
                            Some("end_turn".into()),
                            history.to_vec(),
                            history.snapshot(),
                        )),
                        terminal,
                    }
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
