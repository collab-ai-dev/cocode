//! Tool execution branch helpers for the session loop.

use std::sync::Arc;

use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_tool_runtime::ToolUseContext;
use coco_types::TokenUsage;

use crate::ContinueReason;
use crate::QueryResult;
use crate::emit::emit_protocol;
use crate::engine::QueryEngine;
use crate::engine_finalize_turn::TurnContinuation;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopServices;
use crate::engine_loop_state::LoopTurnState;
use crate::engine_result::make_query_result;
use crate::tool_call_runner::ToolCallRunner;

pub(crate) enum ToolExecutionBranch {
    ContinueLoop,
    Return(Box<QueryResult>),
}

/// #3 / TS query.ts:1566 `toolUseBlocks.some(b => b.name === SLEEP_TOOL_NAME)`:
/// did the just-executed batch include a Sleep tool? Gates the
/// `later`-priority command-queue drain at the turn boundary.
fn tool_batch_ran_sleep(tool_calls: &[ToolCallPart]) -> bool {
    tool_calls
        .iter()
        .any(|tc| tc.tool_name == coco_types::ToolName::Sleep.as_str())
}

#[allow(clippy::too_many_arguments)]
impl QueryEngine {
    pub(crate) async fn execute_or_finalize_tool_calls(
        &self,
        consts: &LoopConstants,
        acc: &mut LoopAccumulator,
        turn_state: &mut LoopTurnState,
        response_text: String,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        hook_tx_opt: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        state_tracker: &crate::session_state::SessionStateTracker,
        services: &LoopServices,
        cycle_turn_id: Option<coco_types::TurnId>,
        usage: TokenUsage,
        parsed_stop_reason: Option<coco_messages::StopReason>,
        tool_calls: &[ToolCallPart],
        messages_snapshot: Arc<Vec<Arc<coco_messages::Message>>>,
        opened_runtime_snapshot: &coco_inference::ModelRuntimeSnapshot,
        streaming_ctx: Option<Arc<ToolUseContext>>,
        streaming_executed: bool,
        streaming_control_prevent: Option<String>,
    ) -> ToolExecutionBranch {
        if streaming_executed {
            if let (Some(state), Some(ctx)) = (self.app_state.as_ref(), streaming_ctx.as_ref()) {
                let permission_mode_after_tools = state.read().await.permissions.mode;
                if permission_mode_after_tools != Some(ctx.permission_context.mode)
                    && let Some(mode) = permission_mode_after_tools
                {
                    let _delivered = emit_protocol(
                        event_tx,
                        coco_types::ServerNotification::PermissionModeChanged(
                            coco_types::PermissionModeChangedParams {
                                mode,
                                bypass_available: ctx.permission_context.bypass_available,
                            },
                        ),
                    )
                    .await;
                }
            }
            if let Some(ref c) = streaming_ctx {
                self.drain_nested_memory_triggers(c).await;
            }
            let continuation = if streaming_control_prevent.is_some() {
                TurnContinuation::Terminal
            } else {
                TurnContinuation::Continuing
            };
            self.finalize_turn_post_tools(
                &mut *history,
                event_tx,
                usage,
                continuation,
                cycle_turn_id.clone(),
                parsed_stop_reason,
                tool_batch_ran_sleep(tool_calls),
            )
            .await;
            if let Some(ref c) = streaming_ctx {
                self.drain_dynamic_skill_triggers(c, &mut *history, event_tx)
                    .await;
            }
            if self.cancel.is_cancelled() {
                return ToolExecutionBranch::Return(Box::new(make_query_result(
                    consts,
                    &*acc,
                    &*turn_state,
                    response_text,
                    /*cancelled*/ true,
                    /*budget_exhausted*/ false,
                    Some("cancelled".into()),
                    history.to_vec(),
                    history.snapshot(),
                )));
            }
            if let Some(stop_reason) = streaming_control_prevent {
                return ToolExecutionBranch::Return(Box::new(make_query_result(
                    consts,
                    &*acc,
                    &*turn_state,
                    response_text,
                    /*cancelled*/ false,
                    /*budget_exhausted*/ false,
                    Some(stop_reason),
                    history.to_vec(),
                    history.snapshot(),
                )));
            }
            turn_state.transition = Some(ContinueReason::NextTurn);
            return ToolExecutionBranch::ContinueLoop;
        }

        let ctx_supports_tool_reference =
            opened_runtime_snapshot
                .model_info
                .as_ref()
                .is_some_and(|info| {
                    info.has_capability(coco_types::Capability::ServerSideToolReference)
                });
        let ctx_supports_client_side_tool_search = opened_runtime_snapshot
            .model_info
            .as_ref()
            .is_some_and(|info| info.has_capability(coco_types::Capability::ClientSideToolSearch));
        let ctx = self
            .tool_context_factory(hook_tx_opt)
            .build(crate::tool_context::ToolContextOverrides {
                user_message_id: Some(consts.user_uuid.clone()),
                progress_tx: Some(services.progress_tx.clone()),
                current_model_id: Some(opened_runtime_snapshot.model_id.clone()),
                current_model_supports_tool_reference: ctx_supports_tool_reference,
                current_model_supports_client_side_tool_search:
                    ctx_supports_client_side_tool_search,
                messages_snapshot: Some(messages_snapshot),
            })
            .await;

        let permission_mode_before_tools = match self.app_state.as_ref() {
            Some(state) => state.read().await.permissions.mode,
            None => None,
        };

        let tool_run_outcome = ToolCallRunner {
            event_tx,
            history: &mut *history,
            ctx: &ctx,
            tool_calls,
            turn: turn_state.turn,
            tools: &self.tools,
            hooks: self.hooks.as_ref(),
            orchestration_ctx: self.orchestration_ctx(),
            hook_tx_opt,
            permission_denials: &mut acc.permission_denials,
            state_tracker,
            permission_bridge: self.permission_bridge.as_ref(),
            session_id: &self.config.session_id,
            cancel: &self.cancel,
            auto_mode_state: self.auto_mode_state.as_ref(),
            denial_tracker: self.denial_tracker.as_ref(),
            model_runtimes: &self.model_runtimes,
            auto_mode_rules: &self.auto_mode_rules,
            app_state: self.app_state.as_ref(),
            permission_rule_handle: &self.permission_rule_handle,
        }
        .run()
        .await;

        if let Some(state) = self.app_state.as_ref() {
            let permission_mode_after_tools = state.read().await.permissions.mode;
            if permission_mode_after_tools != permission_mode_before_tools
                && let Some(mode) = permission_mode_after_tools
            {
                let _delivered = emit_protocol(
                    event_tx,
                    coco_types::ServerNotification::PermissionModeChanged(
                        coco_types::PermissionModeChangedParams {
                            mode,
                            bypass_available: self.config.bypass_permissions_available,
                        },
                    ),
                )
                .await;
            }
        }

        self.drain_nested_memory_triggers(&ctx).await;
        if let Some(data) = tool_run_outcome.structured_output.clone() {
            acc.run_artifacts.structured_output = Some(data);
        }
        acc.run_artifacts.structured_output_attempts = acc
            .run_artifacts
            .structured_output_attempts
            .saturating_add(tool_run_outcome.structured_output_attempts);
        let continuation = if tool_run_outcome.continue_after_tools {
            TurnContinuation::Continuing
        } else {
            TurnContinuation::Terminal
        };
        self.finalize_turn_post_tools(
            &mut *history,
            event_tx,
            usage,
            continuation,
            cycle_turn_id.clone(),
            parsed_stop_reason,
            tool_batch_ran_sleep(tool_calls),
        )
        .await;
        self.drain_dynamic_skill_triggers(&ctx, &mut *history, event_tx)
            .await;
        if tool_run_outcome.permission_aborted {
            return ToolExecutionBranch::Return(Box::new(make_query_result(
                consts,
                &*acc,
                &*turn_state,
                response_text,
                /*cancelled*/ true,
                /*budget_exhausted*/ false,
                Some("permission_abort".into()),
                history.to_vec(),
                history.snapshot(),
            )));
        }
        if self.cancel.is_cancelled() {
            return ToolExecutionBranch::Return(Box::new(make_query_result(
                consts,
                &*acc,
                &*turn_state,
                response_text,
                /*cancelled*/ true,
                /*budget_exhausted*/ false,
                Some("cancelled".into()),
                history.to_vec(),
                history.snapshot(),
            )));
        }
        if !tool_run_outcome.continue_after_tools {
            let cancelled = self.cancel.is_cancelled();
            return ToolExecutionBranch::Return(Box::new(make_query_result(
                consts,
                &*acc,
                &*turn_state,
                response_text,
                cancelled,
                /*budget_exhausted*/ false,
                if cancelled {
                    Some("cancelled".into())
                } else {
                    tool_run_outcome.stop_reason_override
                },
                history.to_vec(),
                history.snapshot(),
            )));
        }
        turn_state.transition = Some(ContinueReason::NextTurn);
        ToolExecutionBranch::ContinueLoop
    }
}
