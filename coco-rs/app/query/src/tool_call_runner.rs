//! `ToolCallRunner` — owns the tool-call lifecycle for a single
//! assistant batch.
//!
//! Phase 4d-β rewires this to drive
//! [`coco_tool_runtime::ToolExecutor::execute_with`] so outcomes
//! surface through `on_outcome` in I12 order:
//!
//! - Concurrent-safe batches surface in completion order.
//! - Serial unsafe tools surface in execution order.
//! - `EarlyOutcome` barriers (unknown tool / schema / permission /
//!   pre-hook block) land in partition order, never globally first.
//!
//! The per-tool semantic lifecycle is split across:
//!
//! 1. `tool_call_preparer::prepare_pending_tool_calls` — pre-hook,
//!    input rewrite, re-validate, permission (including auto-mode
//!    classifier + bridge).
//! 2. `run_one` (here) — `tool.execute` + PostToolUse /
//!    PostToolUseFailure hooks, via
//!    [`tool_outcome_builder::build_outcome_from_execution`].
//!
//! `on_outcome` then appends the pre-flattened `ordered_messages` to
//! history and surfaces `permission_denial` / `prevent_continuation`
//! back to the engine.

use std::sync::Arc;

use coco_hooks::HookExecutionEvent;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_inference::ModelRuntimeRegistry;
use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_permissions::AutoModeRules;
use coco_tool_runtime::PreparedToolCall;
use coco_tool_runtime::ToolAbortSignal;
use coco_tool_runtime::ToolCallPlan;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolExecutor;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::CoreEvent;
use coco_types::PermissionDenialInfo;
use coco_types::ToolAbortReasonPayload;
use coco_types::ToolAppState;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::emit::emit_stream;
use crate::helpers::ToolCompletionEventMode;
use crate::session_state::SessionStateTracker;
use crate::tool_call_preparer::PendingToolPreparation;
use crate::tool_call_preparer::prepare_pending_tool_calls;
use crate::tool_outcome_builder::RunOneTail;
use crate::tool_outcome_builder::build_outcome_from_execution;

#[derive(Debug, Default)]
pub(crate) struct ToolCallRunOutcome {
    pub continue_after_tools: bool,
    pub stop_reason_override: Option<String>,
    /// Captured `ToolResult::with_structured_output(...)` from the last
    /// success-path outcome that produced one. Threaded back to
    /// `run_session_loop`'s `RunArtifacts` so the SDK result picks it up
    /// without scanning history (which is unsound under compaction).
    pub structured_output: Option<serde_json::Value>,
    /// Number of `StructuredOutput` tool invocations seen this batch
    /// (successful + failed). The engine accumulates these across
    /// turns into `RunArtifacts.structured_output_attempts` for the
    /// retry-cap check.
    pub structured_output_attempts: u32,
    pub permission_aborted: bool,
}

pub(crate) struct ToolCallRunner<'a> {
    pub event_tx: &'a Option<mpsc::Sender<CoreEvent>>,
    pub history: &'a mut MessageHistory,
    pub ctx: &'a ToolUseContext,
    pub tool_calls: &'a [ToolCallPart],
    pub turn: i32,
    pub tools: &'a ToolRegistry,
    pub hooks: Option<&'a Arc<HookRegistry>>,
    pub orchestration_ctx: OrchestrationContext,
    pub hook_tx_opt: Option<&'a mpsc::Sender<HookExecutionEvent>>,
    pub permission_denials: &'a mut Vec<PermissionDenialInfo>,
    pub state_tracker: &'a SessionStateTracker,
    pub permission_bridge: Option<&'a ToolPermissionBridgeRef>,
    pub session_id: &'a str,
    pub cancel: &'a CancellationToken,
    pub auto_mode_state: Option<&'a Arc<coco_permissions::AutoModeState>>,
    pub denial_tracker: Option<&'a Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
    pub model_runtimes: &'a Arc<ModelRuntimeRegistry>,
    pub auto_mode_rules: &'a AutoModeRules,
    pub app_state: Option<&'a Arc<RwLock<ToolAppState>>>,
    pub permission_rule_handle: &'a coco_tool_runtime::PermissionRuleHandleRef,
}

impl<'a> ToolCallRunner<'a> {
    pub(crate) async fn run(self) -> ToolCallRunOutcome {
        let tool_names: Vec<&str> = self
            .tool_calls
            .iter()
            .map(|c| c.tool_name.as_str())
            .collect();
        info!(
            turn = self.turn,
            session_id = %self.session_id,
            tool_count = self.tool_calls.len(),
            tools = ?tool_names,
            "executing tool batch"
        );

        // 1. Per-tool preparation (pre-hook + permission + re-validate).
        //    `prepare_pending_tool_calls` emits ToolUseQueued for each
        //    committed tool_use and completes error tool results for
        //    any call that fails preparation (unknown tool / invalid
        //    input / hook block / permission denial).
        let (pending, tool_result_contexts, permission_aborted) =
            prepare_pending_tool_calls(PendingToolPreparation {
                event_tx: self.event_tx,
                history: self.history,
                ctx: self.ctx,
                tool_calls: self.tool_calls,
                tools: self.tools,
                hooks: self.hooks,
                orchestration_ctx: self.orchestration_ctx.clone(),
                hook_tx_opt: self.hook_tx_opt,
                permission_denials: self.permission_denials,
                state_tracker: self.state_tracker,
                permission_bridge: self.permission_bridge,
                session_id: self.session_id,
                cancel: self.cancel,
                auto_mode_state: self.auto_mode_state,
                denial_tracker: self.denial_tracker,
                model_runtimes: self.model_runtimes,
                auto_mode_rules: self.auto_mode_rules,
                completion_event_mode: ToolCompletionEventMode::Emit,
                deferred_tool_completions: None,
            })
            .await;

        // 2. Build `Vec<ToolCallPlan>` from the pre-validated pending
        //    calls. Every plan here is `Runnable` — calls that failed
        //    preparation have already been completed by the preparer
        //    and do not reach this point.
        //
        //    `model_index` is the call's position in the pending
        //    list, which matches the tool_use position in the
        //    assistant message (the preparer iterates
        //    `self.tool_calls` in model order and skips only
        //    early-completed entries).
        let mut plans: Vec<ToolCallPlan> = Vec::with_capacity(pending.len());
        for (idx, pending_call) in pending.into_iter().enumerate() {
            let tool_id = pending_call.tool.id();
            plans.push(ToolCallPlan::Runnable(PreparedToolCall {
                tool_use_id: pending_call.tool_use_id,
                tool_id,
                tool: pending_call.tool,
                parsed_input: pending_call.input,
                is_concurrency_safe: pending_call.is_concurrency_safe,
                model_index: idx,
                // The batch runner threads approval metadata into the execute
                // ctx from `tool_result_contexts` (see `run_one` below), so the
                // plan itself need not carry it.
                permission_resolution_detail: None,
                approval_feedback: None,
            }));
        }

        // 3. Emit ToolUseStarted for each Runnable plan before
        //    dispatching. This keeps the SDK stream shape unchanged
        //    from the legacy path — consumers see Started before
        //    Completed as before.
        for plan in &plans {
            if let ToolCallPlan::Runnable(prepared) = plan {
                let tool_name = tool_result_contexts
                    .get(&prepared.tool_use_id)
                    .map(|c| c.tool_name.clone())
                    .unwrap_or_else(|| prepared.tool.name().to_string());
                let _delivered = emit_stream(
                    self.event_tx,
                    crate::AgentStreamEvent::ToolUseStarted {
                        call_id: prepared.tool_use_id.clone(),
                        name: tool_name,
                        batch_id: None,
                    },
                )
                .await;
            }
        }

        // 4. Drive the scheduler. `run_one` executes the tool +
        //    post-hook and builds `UnstampedToolCallOutcome`;
        //    `on_outcome` appends ordered_messages to history and
        //    records prevent_continuation.
        //
        //    `on_outcome` is `FnMut` (single-threaded, called
        //    synchronously from the executor), so it captures
        //    mutable references directly — no Mutex ceremony, and
        //    no `.unwrap()` for lock acquisition. `run_one` is
        //    `Fn + Sync` (called concurrently via
        //    `FuturesUnordered`), so it only captures immutable
        //    data.
        let shared_ctx = self.ctx;
        let executor = create_executor(self.app_state, self.permission_rule_handle, self.event_tx)
            .with_turn_abort(shared_ctx.abort.turn_signal());
        let hooks = self.hooks;
        let orchestration_ctx = self.orchestration_ctx.clone();
        let hook_tx = self.hook_tx_opt;
        let contexts = &tool_result_contexts;
        let event_tx = self.event_tx;

        let mut control = Control::default();
        if permission_aborted {
            control.continue_after_tools = false;
            control.permission_aborted = true;
        }
        let mut commit_log: Vec<PendingToolCommit> = Vec::new();

        executor
            .execute_with(
                plans,
                |prepared, runtime| {
                    let orchestration_ctx = orchestration_ctx.clone();
                    async move {
                        let ctx_entry = contexts.get(&prepared.tool_use_id);
                        let tool_name = ctx_entry
                            .map(|c| c.tool_name.clone())
                            .unwrap_or_else(|| prepared.tool.name().to_string());
                        let effective_input = ctx_entry
                            .map(|c| c.effective_input.clone())
                            .unwrap_or_else(|| prepared.parsed_input.clone());
                        let mut call_ctx =
                            shared_ctx.clone_for_tool_call(prepared.tool_use_id.clone());
                        call_ctx.abort = runtime.abort.clone();
                        // `progress_tx` is inherited from the base ctx via
                        // `clone_for_tool_call`. Do NOT overwrite it with
                        // `runtime.progress_tx` (always `None` — the executor
                        // carries no progress sink), or foreground Bash loses
                        // real-time `ToolProgress` streaming to the TUI.
                        call_ctx.approval_feedback =
                            ctx_entry.and_then(|c| c.approval_feedback.clone());
                        call_ctx.permission_resolution_detail =
                            ctx_entry.and_then(|c| c.permission_resolution_detail.clone());

                        // Pre-execute abort guard: when the turn is already
                        // aborted before the tool runs,
                        // emit a PreExecutionCancelled outcome (EarlyReturn path,
                        // no PostToolUseFailure hooks) carrying CANCEL_MESSAGE.
                        // Without this the `select!` below would resolve the
                        // cancel arm into a mid-execute ExecutionCancelled, which
                        // wrongly fires failure hooks for a tool that never ran.
                        if call_ctx.abort.is_aborted() {
                            return crate::tool_outcome_builder::build_early_outcome(
                                prepared.tool_use_id.clone(),
                                prepared.tool_id.clone(),
                                &tool_name,
                                prepared.model_index,
                                coco_tool_runtime::ToolCallErrorKind::PreExecutionCancelled,
                                coco_messages::CANCEL_MESSAGE,
                                None,
                            );
                        }

                        // Execute the tool under cancellation.
                        let execute_result = tokio::select! {
                            r = prepared.tool.execute(effective_input.as_value().clone(), &call_ctx) => r,
                            () = call_ctx.abort.cancelled() => Err(tool_error_from_abort(&call_ctx.abort)),
                        };

                        build_outcome_from_execution(RunOneTail {
                            tool_use_id: prepared.tool_use_id.clone(),
                            tool_id: prepared.tool_id.clone(),
                            tool_name,
                            model_index: prepared.model_index,
                            tool: prepared.tool,
                            effective_input: effective_input.into_value(),
                            execute_result,
                            hooks,
                            orchestration_ctx,
                            hook_tx,
                            // Tool Result Budget Level 1 session dir.
                            tool_result_session_dir: shared_ctx.tool_result_session_dir.clone(),
                        })
                        .await
                    }
                },
                |outcome| {
                    let ctx_entry = contexts.get(outcome.tool_use_id());
                    let (tool_name, is_error) = match (ctx_entry, outcome.error_kind()) {
                        (Some(c), _) => (c.tool_name.clone(), outcome.error_kind().is_some()),
                        (None, _) => (
                            outcome.tool_id().to_string(),
                            outcome.error_kind().is_some(),
                        ),
                    };

                    // Capture Completed event data for emission after
                    // the executor finishes driving — we can't
                    // `.await` from the sync `on_outcome` callback.
                    let output_text = render_completed_output(&outcome);
                    let completed_event = PendingCompletedEvent {
                        call_id: outcome.tool_use_id().to_string(),
                        tool_name,
                        output: output_text,
                        is_error,
                    };

                    // Update control signals from prevent_continuation.
                    if let Some(reason) = outcome.prevent_continuation() {
                        control.continue_after_tools = false;
                        if control.stop_reason_override.is_none() {
                            control.stop_reason_override = Some(reason.to_string());
                        }
                    }

                    // Append ordered_messages verbatim — the runner
                    // has already flattened per MCP / non-MCP / path
                    // rules.
                    let parts = outcome.into_parts();
                    if matches!(
                        parts.tool_id,
                        coco_types::ToolId::Builtin(coco_types::ToolName::StructuredOutput)
                    ) {
                        control.structured_output_attempts =
                            control.structured_output_attempts.saturating_add(1);
                    }
                    if let Some(data) = parts.structured_output.clone() {
                        control.structured_output = Some(data);
                    }
                    commit_log.push(PendingToolCommit {
                        ordered_messages: parts.ordered_messages,
                        completed_event,
                    });
                },
            )
            .await;

        // 5. Commit ordered messages and emit Completed events now
        //    that we're outside the executor's sync on_outcome
        //    boundary. Use history_push_and_emit so the TUI and SDK
        //    see each tool result through MessageAppended.
        for commit in commit_log {
            let PendingToolCommit {
                ordered_messages,
                completed_event,
            } = commit;
            for msg in ordered_messages {
                crate::history_sync::history_push_and_emit(self.history, msg, self.event_tx).await;
            }
            let event = completed_event;
            let _delivered = emit_stream(
                event_tx,
                crate::AgentStreamEvent::ToolUseCompleted {
                    call_id: event.call_id,
                    name: event.tool_name,
                    output: event.output,
                    is_error: event.is_error,
                },
            )
            .await;
        }

        control.into_outcome()
    }
}

/// Render the text payload surfaced in `ToolUseCompleted.output`
/// events so SDK consumers see the same string the legacy path
/// emitted.
///
/// Success paths carry the flattened tool_result text; failure /
/// early-return paths carry the synthetic error message.
fn render_completed_output(outcome: &coco_tool_runtime::ToolCallOutcome) -> String {
    // Extract the tool_result body from the first ordered message
    // that is a ToolResult. Matches the legacy processor which
    // serialized the tool output into the event.
    for msg in outcome.ordered_messages() {
        if let coco_messages::Message::ToolResult(tr) = msg
            && let coco_messages::LlmMessage::Tool { content, .. } = &tr.message
        {
            for part in content {
                if let coco_messages::ToolContent::ToolResult(r) = part {
                    match &r.output {
                        coco_llm_types::ToolResultContent::Text { value, .. } => {
                            return value.clone();
                        }
                        coco_llm_types::ToolResultContent::ErrorText { value, .. } => {
                            return value.clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    String::new()
}

fn tool_error_from_abort(signal: &ToolAbortSignal) -> ToolError {
    match signal.reason() {
        Some(ToolAbortReasonPayload::SiblingError { failed_tool }) => ToolError::ExecutionFailed {
            message: format!("Cancelled: parallel tool call {failed_tool} errored"),
            display_data: None,
            source: None,
        },
        Some(ToolAbortReasonPayload::SelfAbort { message }) => ToolError::ExecutionFailed {
            message,
            display_data: None,
            source: None,
        },
        Some(ToolAbortReasonPayload::Turn { .. }) | None => ToolError::Cancelled,
    }
}

#[derive(Debug)]
struct PendingCompletedEvent {
    call_id: String,
    tool_name: String,
    output: String,
    is_error: bool,
}

#[derive(Debug)]
struct PendingToolCommit {
    ordered_messages: Vec<coco_messages::Message>,
    completed_event: PendingCompletedEvent,
}

#[derive(Debug)]
struct Control {
    continue_after_tools: bool,
    stop_reason_override: Option<String>,
    structured_output: Option<serde_json::Value>,
    structured_output_attempts: u32,
    permission_aborted: bool,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            continue_after_tools: true,
            stop_reason_override: None,
            structured_output: None,
            structured_output_attempts: 0,
            permission_aborted: false,
        }
    }
}

impl Control {
    fn into_outcome(self) -> ToolCallRunOutcome {
        ToolCallRunOutcome {
            continue_after_tools: self.continue_after_tools,
            stop_reason_override: self.stop_reason_override,
            structured_output: self.structured_output,
            structured_output_attempts: self.structured_output_attempts,
            permission_aborted: self.permission_aborted,
        }
    }
}

fn create_executor(
    app_state: Option<&Arc<RwLock<ToolAppState>>>,
    permission_rule_handle: &coco_tool_runtime::PermissionRuleHandleRef,
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
) -> ToolExecutor {
    let base = match app_state {
        Some(arc) => ToolExecutor::new().with_app_state(arc.clone()),
        None => ToolExecutor::new(),
    };
    let base = base.with_permission_rule_handle(permission_rule_handle.clone());
    match event_tx {
        Some(tx) => base.with_event_sink(tx.clone()),
        None => base,
    }
}
