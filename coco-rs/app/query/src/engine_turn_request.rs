//! Turn-entry request preparation for the session loop.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_system_reminder::count_human_turns;
use coco_tool_runtime::StreamingHandle;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::call_plan::PreparedToolCall;
use coco_tool_runtime::call_plan::RunOneRuntime;
use coco_tool_runtime::call_plan::UnstampedToolCallOutcome;
use tracing::info;
use tracing::warn;

use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopServices;
use crate::engine_loop_state::LoopTurnState;

pub(crate) type StreamingRunFuture = Pin<Box<dyn Future<Output = UnstampedToolCallOutcome> + Send>>;
pub(crate) type StreamingRunFn =
    Box<dyn Fn(PreparedToolCall, RunOneRuntime) -> StreamingRunFuture + Send + Sync>;
pub(crate) type SessionStreamingHandle = StreamingHandle<StreamingRunFn, StreamingRunFuture>;

pub(crate) struct PreparedTurnRequest {
    pub(crate) params: QueryParams,
    pub(crate) active_snapshot: coco_inference::ModelRuntimeSnapshot,
    pub(crate) messages_snapshot: Arc<Vec<Arc<Message>>>,
    pub(crate) streaming_ctx: Option<Arc<ToolUseContext>>,
    pub(crate) streaming_handle: Option<SessionStreamingHandle>,
    pub(crate) streaming_model_index: usize,
}

#[allow(clippy::too_many_arguments)]
impl QueryEngine {
    pub(crate) async fn enter_turn_and_prepare_request(
        &self,
        consts: &LoopConstants,
        acc: &LoopAccumulator,
        turn_state: &mut LoopTurnState,
        services: &mut LoopServices,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
        hook_tx_opt: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        cycle_turn_id: Option<coco_types::TurnId>,
        turn_id: &str,
    ) -> PreparedTurnRequest {
        let live_permission_mode = match self.app_state.as_ref() {
            Some(state) => state
                .read()
                .await
                .permissions
                .mode
                .unwrap_or(self.config.permission_mode),
            None => self.config.permission_mode,
        };
        let use_plan_runtime = live_permission_mode == coco_types::PermissionMode::Plan
            && !crate::engine_helpers::most_recent_assistant_exceeds(
                history.as_slice(),
                self.config
                    .plan_mode_settings
                    .plan_model_fallback_threshold_tokens,
            );
        let can_use_plan_runtime = matches!(
            services.main_source,
            ModelRuntimeSource::Role(coco_types::ModelRole::Main)
        );
        let (active_runtime, active_source) = if use_plan_runtime && can_use_plan_runtime {
            let source = ModelRuntimeSource::Role(coco_types::ModelRole::Plan);
            let runtime = self
                .model_runtimes
                .runtime_for_source(source.clone())
                .unwrap_or_else(|e| {
                    warn!(
                        error = %e,
                        "Plan model runtime registry lookup failed; using engine runtime"
                    );
                    services.main_runtime.clone()
                });
            (runtime, source)
        } else {
            (services.main_runtime.clone(), services.main_source.clone())
        };
        services.set_active_runtime(active_runtime, active_source);
        let active_snapshot = services.snapshot();

        let session_turn = count_human_turns(history.as_slice());
        let (last_compact_run_id, turns_since_last_compact) = self
            .last_compact_state
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .map(|prev| (Some(prev.run_id), Some(prev.turn_counter)))
            .unwrap_or((None, None));
        info!(
            turn = turn_state.turn,
            attempt = turn_state.attempt,
            turn_id = %turn_id,
            cycle_turn_id = ?cycle_turn_id.as_ref().map(coco_types::TurnId::as_str),
            session_turn,
            last_compact_run_id = ?last_compact_run_id,
            turns_since_last_compact = ?turns_since_last_compact,
            history_len = history.len(),
            active_model = active_snapshot.model_id,
            live_permission_mode = ?live_permission_mode,
            configured_permission_mode = ?self.config.permission_mode,
            use_plan_runtime,
            "turn start (per-round; cycle TurnStarted was emitted by run_internal_with_messages)"
        );

        let app_state_snapshot = self
            .run_turn_reminder_pipeline(crate::engine_turn_reminders::TurnReminderContext {
                history: &mut *history,
                plan_reminder: &mut services.plan,
                orchestrator: &services.reminders,
                last_user_input_uuid: &mut turn_state.reminder_last_user_input_uuid,
                total_usage: &acc.total_usage,
                cost_tracker: &acc.cost_tracker,
                todo_key: &consts.todo_key,
                context_window: consts.context_window,
                effective_window: consts.effective_window,
                event_tx,
            })
            .await;

        let crate::engine_prompt::BuiltPrompt {
            prompt,
            messages_snapshot,
        } = self.build_prompt(history).await;
        let tool_defs = self.build_tool_definitions(&app_state_snapshot).await;

        let context_management = if active_snapshot.supports_server_side_context_edits {
            let mut pending = self.pending_reactive_context_management.lock().await;
            if let Some(v) = pending.take() {
                Some(v)
            } else {
                drop(pending);
                let opts = coco_compact::ApiContextOptions::from_config(
                    &self.config.compact.api_native,
                    /*has_thinking*/ self.config.thinking_level.is_some(),
                    /*is_redact_thinking_active*/ false,
                    /*clear_all_thinking*/ false,
                );
                let strategies = coco_compact::get_api_context_management(&opts);
                coco_compact::encode_anthropic_context_management(&strategies)
            }
        } else {
            None
        };

        let query_source = self.query_source_label();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let last_ms = self
            .last_assistant_ms
            .load(std::sync::atomic::Ordering::Acquire);
        let time_since_last_assistant_ms = if last_ms > 0 {
            Some((now_ms - last_ms).max(0))
        } else {
            None
        };

        let mut params = QueryParams {
            prompt,
            max_tokens: None,
            thinking_level: self.config.thinking_level.clone(),
            // config#247: fast-mode support is capability-driven and
            // provider-agnostic — gate on the resolved model's
            // `Capability::FastMode`, not a hardcoded id substring. The owning
            // provider crate translates the flag to its wire option (Anthropic
            // → `speed=fast` beta, set in build_call_options).
            fast_mode: self.config.fast_mode
                && active_snapshot
                    .model_info
                    .as_ref()
                    .is_some_and(|info| info.has_capability(coco_types::Capability::FastMode)),
            tools: if tool_defs.is_empty() {
                None
            } else {
                Some(tool_defs)
            },
            tool_choice: None,
            context_management,
            query_source: Some(query_source.to_string()),
            agent_id: self.config.agent_id.clone(),
            time_since_last_assistant_ms,
            agentic: true,
            cache: self.config.prompt_cache.clone(),
            stop_sequences: None,
            response_format: None,
            // Interruptible backoff: a user interrupt cancels a long capacity
            // retry instead of waiting it out.
            cancel: Some(self.cancel.clone()),
            // Set below once the active model snapshot is known.
            wire_tap: None,
        };

        let streaming_ctx: Option<Arc<ToolUseContext>> = if self.config.streaming_tool_execution {
            let current_supports_tool_reference =
                active_snapshot.model_info.as_ref().is_some_and(|info| {
                    info.has_capability(coco_types::Capability::ServerSideToolReference)
                });
            let current_supports_client_side_tool_search =
                active_snapshot.model_info.as_ref().is_some_and(|info| {
                    info.has_capability(coco_types::Capability::ClientSideToolSearch)
                });
            let base = self
                .tool_context_factory(hook_tx_opt)
                .build(crate::tool_context::ToolContextOverrides {
                    user_message_id: Some(consts.user_uuid.clone()),
                    progress_tx: Some(services.progress_tx.clone()),
                    current_model_id: Some(services.current_model_id()),
                    current_model_supports_tool_reference: current_supports_tool_reference,
                    current_model_supports_client_side_tool_search:
                        current_supports_client_side_tool_search,
                    messages_snapshot: Some(messages_snapshot.clone()),
                })
                .await;
            Some(Arc::new(base))
        } else {
            None
        };
        let streaming_handle = streaming_ctx.as_ref().map(|ctx_arc| {
            let executor_base = coco_tool_runtime::ToolExecutor::new()
                .with_turn_abort(ctx_arc.abort.turn_signal());
            let executor_with_state = match self.app_state.as_ref() {
                Some(state) => executor_base.with_app_state(state.clone()),
                None => executor_base,
            };
            let executor = Arc::new(
                executor_with_state.with_permission_rule_handle(self.permission_rule_handle.clone()),
            );
            let ctx_for_closure = ctx_arc.clone();
            let hooks_for_closure = self.hooks.clone();
            let orchestration_for_closure = self.orchestration_ctx();
            let hook_tx_for_closure = hook_tx_opt.cloned();
            let run_one: StreamingRunFn = Box::new(move |prepared, runtime| {
                let ctx = ctx_for_closure.clone();
                let hooks = hooks_for_closure.clone();
                let orchestration_ctx = orchestration_for_closure.clone();
                let hook_tx = hook_tx_for_closure.clone();
                Box::pin(async move {
                    let effective_input = prepared.parsed_input.clone();
                    let mut call_ctx = ctx.clone_for_tool_call(prepared.tool_use_id.clone());
                    call_ctx.abort = runtime.abort.clone();
                    // `progress_tx` is inherited from the base streaming ctx via
                    // `clone_for_tool_call`. Do NOT overwrite it with
                    // `runtime.progress_tx` (always `None`), or foreground Bash
                    // loses real-time `ToolProgress` streaming to the TUI.
                    // Thread per-call approval metadata into the execute ctx so
                    // the streaming path matches the batch runner. Without this,
                    // tools that branch on the user's choice (ExitPlanMode's
                    // clear-context / mode selection) saw `None` under streaming.
                    call_ctx.permission_resolution_detail =
                        prepared.permission_resolution_detail.clone();
                    call_ctx.approval_feedback = prepared.approval_feedback.clone();
                    let execute_result = tokio::select! {
                        r = prepared.tool.execute(effective_input.as_value().clone(), &call_ctx) => r,
                        () = call_ctx.abort.cancelled() => Err(coco_tool_runtime::ToolError::Cancelled),
                    };
                    crate::tool_outcome_builder::build_outcome_from_execution(
                        crate::tool_outcome_builder::RunOneTail {
                            tool_use_id: prepared.tool_use_id.clone(),
                            tool_id: prepared.tool_id.clone(),
                            tool_name: prepared.tool.name().to_string(),
                            model_index: prepared.model_index,
                            tool: prepared.tool,
                            effective_input: effective_input.into_value(),
                            execute_result,
                            hooks: hooks.as_ref(),
                            orchestration_ctx,
                            hook_tx: hook_tx.as_ref(),
                            tool_result_session_dir: ctx.tool_result_session_dir.clone(),
                        },
                    )
                    .await
                }) as StreamingRunFuture
            });
            executor.streaming_handle(run_one)
        });

        let effective_max_tokens =
            crate::engine_recovery::effective_max_tokens(&active_snapshot, turn_state);
        params.max_tokens = effective_max_tokens;

        // Attach the per-session wire dumper (no-op unless
        // `diagnostics.wire_dump` is enabled). The recorder rides
        // `params.wire_tap` → `CallOptions.wire_tap` → the transport tap;
        // a concrete handle is parked on `turn_state` so `consume_stream`
        // can report the typed outcome via `finish`.
        if let Some(wire_dump) = self.config.wire_dump.as_ref() {
            let recorder = wire_dump.begin(coco_wire_dump::WireTurnCtx {
                turn_id,
                provider: &active_snapshot.provider,
                model: &active_snapshot.model_id,
            });
            // The recorder is provider-agnostic; a thin adapter bridges it
            // to the transport's `WireTap` sink.
            params.wire_tap = Some(std::sync::Arc::new(
                crate::wire_tap_adapter::WireTapAdapter(recorder.clone()),
            ));
            turn_state.wire_recorder = Some(recorder);
        }

        tracing::debug!(
            turn = turn_state.turn,
            turn_id = %turn_id,
            provider = active_snapshot.provider,
            model_id = active_snapshot.model_id,
            max_tokens = ?effective_max_tokens,
            tool_count = params.tools.as_ref().map(Vec::len).unwrap_or(0),
            prompt_messages = params.prompt.len(),
            agentic = params.agentic,
            probing = false,
            "opening LLM stream"
        );

        PreparedTurnRequest {
            params,
            active_snapshot,
            messages_snapshot,
            streaming_ctx,
            streaming_handle,
            streaming_model_index: 0,
        }
    }
}
