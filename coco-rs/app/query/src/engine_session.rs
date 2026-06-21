//! Session lifecycle impl for [`QueryEngine`].
//!
//! Owns the public entry points (`run`, `run_with_events`,
//! `run_with_messages`) and the orchestration around `run_session_loop`:
//! emitting `SessionStarted` / `SessionStateChanged(Running)` / `Idle` /
//! `SessionResult`, plus the hook-event forwarder bridge that carries
//! `HookExecutionEvent` from `coco-hooks` into `CoreEvent::Protocol`.
//!
//! Every consumer of the SDK / TUI sees the same envelope of session
//! events, regardless of whether the inner loop succeeded or errored.
//!
//! Extracted from `engine.rs` so the multi-turn loop file can stay
//! focused on per-turn mechanics.

use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

use coco_hooks::orchestration::OrchestrationContext;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::create_user_message;
use coco_types::TokenUsage;
use coco_types::TurnId;

use crate::CoreEvent;
use crate::ServerNotification;
use crate::config::QueryResult;
use crate::emit::emit_protocol;
use crate::emit::emit_protocol_owned;
use crate::engine::QueryEngine;
use crate::error_code::error_code_from_boxed_error;
use crate::helpers::extract_last_assistant_text;
use crate::helpers::hook_outcome_to_status;
use crate::session_state::SessionStateTracker;

impl QueryEngine {
    /// Run the agent loop with event streaming from a text prompt.
    ///
    /// **I-1 protocol**: because this entry point CREATES the user
    /// message internally (vs `run_with_messages` where the caller
    /// pre-builds + pre-emits), it is the "authoritative introducer"
    /// for that message and must emit `MessageAppended` itself.
    /// Without this, consumers folding `MessageAppended` into a
    /// transcript view (TUI, test harnesses) never see the user
    /// cell — production tui_runner has its own
    /// `history_push_and_emit` step which feeds `run_with_messages`,
    /// so that path was unaffected, but every test using
    /// `run_with_events` silently lost user-message rendering until
    /// this commit.
    ///
    /// `cycle_turn_id` is the lifecycle id shared between the
    /// `TurnStarted` event this function emits and every `TurnEnded`
    /// event the engine (or this function on Err) emits for the same
    /// user-prompt cycle. Callers (`tui_runner` / `sdk_runner` / SDK
    /// `TurnRunner`) generate it via [`TurnId::generate`] so they can
    /// emit late-cancel `TurnEnded(Interrupted)` with the matching id.
    pub async fn run_with_events(
        &self,
        user_prompt: &str,
        event_tx: tokio::sync::mpsc::Sender<CoreEvent>,
        cycle_turn_id: TurnId,
    ) -> Result<QueryResult, coco_error::BoxedError> {
        let user_msg = std::sync::Arc::new(create_user_message(user_prompt));
        let event_tx_opt = Some(event_tx.clone());
        let _delivered = emit_protocol(
            &event_tx_opt,
            coco_types::ServerNotification::MessageAppended {
                message: user_msg.clone(),
                session_id: self.config.session_id.clone(),
                agent_id: self.config.agent_id.clone(),
            },
        )
        .await;
        self.run_internal_with_messages(vec![user_msg], Some(event_tx), Some(cycle_turn_id))
            .await
    }

    /// Run the agent loop with pre-built messages (user + attachment messages).
    pub async fn run_with_messages(
        &self,
        messages: Vec<std::sync::Arc<Message>>,
        event_tx: tokio::sync::mpsc::Sender<CoreEvent>,
        cycle_turn_id: TurnId,
    ) -> Result<QueryResult, coco_error::BoxedError> {
        if messages.is_empty() {
            return Err(Box::new(coco_error::PlainError::new(
                "No messages to process",
                coco_error::StatusCode::InvalidArguments,
            )));
        }
        self.run_internal_with_messages(messages, Some(event_tx), Some(cycle_turn_id))
            .await
    }

    /// Run the agent loop with pre-built messages and no event streaming.
    pub async fn run_with_messages_no_events(
        &self,
        messages: Vec<std::sync::Arc<Message>>,
    ) -> Result<QueryResult, coco_error::BoxedError> {
        if messages.is_empty() {
            return Err(Box::new(coco_error::PlainError::new(
                "No messages to process",
                coco_error::StatusCode::InvalidArguments,
            )));
        }
        self.run_internal_with_messages(messages, None, None).await
    }

    /// Run the agent loop with an initial user prompt (no event streaming).
    pub async fn run(&self, user_prompt: &str) -> Result<QueryResult, coco_error::BoxedError> {
        let user_msg = std::sync::Arc::new(create_user_message(user_prompt));
        self.run_internal_with_messages(vec![user_msg], None, None)
            .await
    }

    /// Core internal implementation: user + attachment messages.
    ///
    /// First message is the user message (used for file history snapshot UUID).
    /// Subsequent messages are attachment messages (is_meta=true, system-reminder wrapped).
    ///
    /// Session lifecycle sequence:
    /// 1. SessionStarted  (if bootstrap attached)
    /// 2. SessionStateChanged(Running)
    /// 3. run_session_loop: turn-by-turn work
    /// 4. SessionStateChanged(Idle)
    /// 5. SessionResult (success or error subtype)
    ///
    /// Steps 1/2/4/5 fire regardless of success or error so SDK consumers
    /// always see a complete session envelope.
    #[tracing::instrument(
        skip_all,
        name = "session",
        fields(
            session_id = %self.config.session_id,
            agent_id = ?self.config.agent_id,
            model_id = %self.config.model_id,
        ),
    )]
    pub(crate) async fn run_internal_with_messages(
        &self,
        turn_messages: Vec<std::sync::Arc<Message>>,
        event_tx: Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        cycle_turn_id: Option<TurnId>,
    ) -> Result<QueryResult, coco_error::BoxedError> {
        info!(
            turn_message_count = turn_messages.len(),
            streaming_tools = self.config.streaming_tool_execution,
            max_turns = ?self.config.max_turns,
            query_source = %self.query_source_label(),
            fork_label = ?self.config.fork_label,
            configured_permission_mode = ?self.config.permission_mode,
            "session entering agent loop"
        );

        // Single choke point for all SessionStateChanged emissions. Dedupes
        // consecutive identical states so the wire stream sees exactly one
        // emission per real edge. See plan file WS-4.
        let state_tracker = SessionStateTracker::new();

        // SessionStarted must happen before anything that can error.
        self.emit_session_started(&event_tx).await;

        // Running — agent is actively processing.
        state_tracker
            .transition_to(coco_types::SessionState::Running, &event_tx)
            .await;

        // TurnStarted — one per logical user-prompt cycle. The id is
        // owned by the runner so that late-cancel emits from there
        // share the same id; if a caller skipped the event_tx (no-events
        // path), there's nothing to pair against and we skip emission.
        if let (Some(tx), Some(id)) = (event_tx.as_ref(), cycle_turn_id.as_ref()) {
            let _ = tx
                .send(CoreEvent::Protocol(ServerNotification::TurnStarted(
                    coco_types::TurnStartedParams {
                        turn_id: id.clone(),
                    },
                )))
                .await;
        }

        // Set up the Hook → CoreEvent forwarder as a structured child task.
        //
        // The forwarder is a `JoinHandle` owned by this function, cancelled
        // via a child `CancellationToken` off `self.cancel`, and drained at
        // the single exit point below. See plan file WS-5.
        //
        // In Rust we use this child task so orchestration stays independent
        // of the coco-query event type.
        let hook_cancel = self.cancel.child_token();
        // Only emit hook events to the SDK stream when the session was started
        // with the flag. When disabled, skip the forwarder channel entirely so
        // the orchestration layer never sees a sender (cheaper than emitting +
        // dropping).
        let (hook_tx_opt, hook_forwarder_handle) =
            if event_tx.is_some() && self.config.include_hook_events {
                let (hook_event_tx, hook_event_rx) =
                    tokio::sync::mpsc::channel::<coco_hooks::HookExecutionEvent>(64);
                let core_tx = event_tx.clone();
                let handle = tokio::spawn(Self::forward_hook_events(
                    hook_event_rx,
                    core_tx,
                    hook_cancel.clone(),
                ));
                (Some(hook_event_tx), Some(handle))
            } else {
                (None, None)
            };

        // History is owned here so StopFailure can carry the last assistant
        // message text on the error path. On success the QueryResult already
        // exposes `response_text`.
        let mut history = MessageHistory::new();
        // Stamp F9 envelope so every emit from this engine invocation
        // carries the active session + agent identity.
        history.set_envelope(self.config.session_id.clone(), self.config.agent_id.clone());
        let (result, accumulated_usage) = self
            .run_session_loop(
                turn_messages,
                event_tx.clone(),
                &state_tracker,
                hook_tx_opt.clone(),
                &mut history,
                cycle_turn_id.clone(),
            )
            .await;

        // Drain the hook forwarder before emitting Idle/SessionResult so any
        // in-flight hook events land on the wire before the session
        // terminator. Order matters: drop the sender FIRST so the
        // forwarder's `rx.recv()` sees channel-closed; then await the
        // handle with a bounded timeout so a runaway hook can't wedge
        // shutdown.
        drop(hook_tx_opt);
        if let Some(handle) = hook_forwarder_handle {
            const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
            match tokio::time::timeout(DRAIN_TIMEOUT, handle).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    warn!(error = %join_err, "hook forwarder task panicked");
                }
                Err(_) => {
                    warn!("hook forwarder drain timed out; cancelling");
                    hook_cancel.cancel();
                }
            }
        }

        // Idle — turn-over signal; emit regardless of outcome.
        state_tracker
            .transition_to(coco_types::SessionState::Idle, &event_tx)
            .await;

        // Subagent finalize: drop this agent's tracking entry from the
        // shared `CacheBreakDetector` so a long-running parent session
        // doesn't accumulate stale subagent snapshots that would push
        // out the main thread's entry under the LRU cap.
        if let Some(agent_id) = self.config.agent_id.as_deref()
            && let Ok(runtime) = self
                .model_runtimes
                .runtime_for_source(self.model_runtime_source.clone())
        {
            coco_inference::ModelRuntime::cleanup_active_agent(runtime, agent_id).await;
        }

        // TurnEnded(Failed) — wire-protocol terminator on the error path
        // so SDK iterators / TUI state machines don't block on `events()`
        // waiting for a `TurnEnded` notification. Fires before
        // `SessionResult` so turn-level consumers see the turn-end signal
        // first. Maps `coco_error::StatusCategory` → typed `ErrorCode`
        // via the central seam in `crate::error_code` so Hub/SDK consumers
        // can filter without parsing the message string. The accumulated
        // usage up to the failure point flows through here — `None` only
        // when the caller provided no event_tx, never as a sentinel for
        // "zero".
        //
        // Cancel-aware: when `self.cancel.is_cancelled()`, the Err is the
        // bubbled cancellation, not a real failure. Skip the Failed emit
        // and let the runner emit Interrupted with the correct
        // `TurnAbortReason` (it owns the turn abort signal). Without
        // this gate the wire stream becomes `… → Failed → Interrupted`
        // for the same cycle — the Failed lights up the TUI error modal
        // milliseconds before Interrupted overrides it.
        if let (Err(e), Some(id)) = (&result, cycle_turn_id.as_ref())
            && !self.cancel.is_cancelled()
        {
            // Inline transcript error row (TS `SystemAPIErrorMessage` parity):
            // a display-only `SystemMessage::ApiError` appended to history so
            // the failure renders as a `⚠ <error>` row in turn order instead
            // of an ephemeral toast / blocking modal. Dropped from the API
            // request by `normalize_messages_for_api` (every `SystemMessage`
            // except `LocalCommand` maps to `None`), so it never re-enters the
            // model context. Skipped when the engine already pushed an
            // api-error signal (abnormal-stop / context-overflow paths) so a
            // single failure surfaces exactly one row.
            let already_reported = history
                .last()
                .is_some_and(|m| coco_messages::predicates::is_api_error_message(m.as_ref()));
            if !already_reported {
                crate::history_sync::history_push_and_emit(
                    &mut history,
                    coco_messages::create_api_error_message(
                        &e.to_string(),
                        /*status_code*/ None,
                    ),
                    &event_tx,
                )
                .await;
            }
            let _ = emit_protocol(
                &event_tx,
                ServerNotification::TurnEnded(coco_types::TurnEndedParams::failed(
                    id.clone(),
                    Some(accumulated_usage),
                    coco_types::ErrorPayload {
                        message: e.to_string(),
                        code: error_code_from_boxed_error(e),
                    },
                )),
            )
            .await;
        }

        // StopFailure — fire-and-forget hooks when the turn ended in an
        // API / runtime error rather than a clean stop. Output and exit
        // codes are intentionally ignored — this is observability only,
        // not a recovery path. We swallow registry-level failures so a
        // misconfigured hook can't suppress the user-visible error.
        if let (Err(e), Some(hooks)) = (&result, &self.hooks) {
            let err_msg = e.to_string();
            let hook_ctx = self.orchestration_ctx();
            let last_text = extract_last_assistant_text(&history);
            let last_assistant_message = (!last_text.is_empty()).then_some(last_text);
            // Without error-classification infrastructure here we pass a single
            // bucket; users match on `error_details` for the raw text.
            if let Err(hook_err) = coco_hooks::orchestration::execute_stop_failure(
                hooks,
                &hook_ctx,
                /*error_label*/ "unknown",
                Some(err_msg.as_str()),
                last_assistant_message.as_deref(),
            )
            .await
            {
                warn!(error = %hook_err, "StopFailure hook execution failed");
            }
        }

        // SessionResult — always emitted. On Err, we synthesize a minimal
        // QueryResult-like view so SDK consumers see a terminal `result` event.
        let params = match &result {
            Ok(qr) => self.build_session_result_params(qr, /*error_messages*/ Vec::new()),
            Err(e) => self.build_session_error_params(e.to_string()),
        };
        match &result {
            Ok(qr) => info!(
                turns = qr.turns,
                duration_ms = qr.duration_ms,
                duration_api_ms = qr.duration_api_ms,
                tokens_in = qr.total_usage.input_tokens.total,
                tokens_out = qr.total_usage.output_tokens.total,
                cancelled = qr.cancelled,
                budget_exhausted = qr.budget_exhausted,
                stop_reason = ?qr.stop_reason,
                "session complete"
            ),
            Err(e) => warn!(
                error = %e,
                "session terminated with error"
            ),
        }
        let _delivered = emit_protocol(
            &event_tx,
            ServerNotification::SessionResult(Box::new(params)),
        )
        .await;

        result
    }

    /// Emit the `SessionStarted` protocol event from attached bootstrap data.
    /// No-op if the engine was not built with `with_session_bootstrap()`.
    pub(crate) async fn emit_session_started(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        let Some(bootstrap) = &self.session_bootstrap else {
            return;
        };
        // Wire format is whatever `PermissionMode`'s serde serialization
        // produces — camelCase matching the `PermissionModeSchema` wire format.
        let permission_mode = serde_json::to_value(self.config.permission_mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "default".into());
        let tools = if bootstrap.tools.is_empty() {
            let stub_ctx = coco_tool_runtime::ToolUseContext::stub_for_filtering(
                self.config.features.clone(),
                self.config.tool_overrides.clone(),
                self.config.tool_filter.clone(),
                self.config.permission_mode,
            )
            .with_active_shell_tool(self.config.active_shell_tool);
            self.tools
                .loaded_tools(&stub_ctx)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        } else {
            bootstrap.tools.clone()
        };
        // Snapshot LSP connectivity for the status bar. Sync read — the
        // adapter's `is_connected()` is backed by an AtomicBool refined
        // by bootstrap prewarm, so this reflects accurate running-state
        // at session-start time. Subsequent runtime changes (server
        // crash, reload) would need a separate notification to update.
        let lsp_active = self.lsp_handle.as_ref().is_some_and(|h| h.is_connected());

        let _delivered = emit_protocol(
            event_tx,
            ServerNotification::SessionStarted(coco_types::SessionStartedParams {
                session_id: self.config.session_id.clone(),
                protocol_version: bootstrap.protocol_version.clone(),
                cwd: bootstrap.cwd.clone(),
                model: self.config.model_id.clone(),
                provider: self
                    .runtime_snapshot()
                    .map(|snapshot| snapshot.provider)
                    .unwrap_or_default(),
                permission_mode,
                tools,
                slash_commands: bootstrap.slash_commands.clone(),
                agents: bootstrap.agents.clone(),
                skills: bootstrap.skills.clone(),
                mcp_servers: bootstrap.mcp_servers.clone(),
                plugins: bootstrap.plugins.clone(),
                api_key_source: bootstrap.api_key_source.clone(),
                betas: bootstrap.betas.clone(),
                version: bootstrap.version.clone(),
                output_style: bootstrap.output_style.clone(),
                fast_mode_state: bootstrap.fast_mode_state,
                lsp_active,
            }),
        )
        .await;
    }

    /// Synthesize a `SessionResultParams` for the error path (when
    /// `run_session_loop` returned `Err`).
    pub(crate) fn build_session_error_params(
        &self,
        error_msg: String,
    ) -> coco_types::SessionResultParams {
        coco_types::SessionResultParams {
            session_id: self.config.session_id.clone(),
            total_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            is_error: true,
            stop_reason: "error_during_execution".into(),
            total_cost_usd: 0.0,
            usage: TokenUsage::default(),
            model_usage: Default::default(),
            permission_denials: Vec::new(),
            result: None,
            errors: vec![error_msg],
            structured_output: None,
            fast_mode_state: None,
            num_api_calls: None,
        }
    }

    /// Build a `SessionResultParams` from a completed `QueryResult`.
    ///
    /// `error_messages` is propagated into the `errors` field;
    /// success results pass an empty Vec.
    pub(crate) fn build_session_result_params(
        &self,
        qr: &QueryResult,
        error_messages: Vec<String>,
    ) -> coco_types::SessionResultParams {
        // Per-model usage aggregated from CostTracker.
        let model_usage = qr
            .cost_tracker
            .model_entries()
            .map(|(key, usage)| {
                (
                    key.display(),
                    coco_types::SessionModelUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        web_search_requests: usage.web_search_requests,
                        cost_usd: usage.total_cost_usd,
                        context_window: self.config.context_window,
                        max_output_tokens: self.config.max_output_tokens,
                    },
                )
            })
            .collect();

        let stop_reason = qr
            .stop_reason
            .clone()
            .unwrap_or_else(|| "end_turn".to_string());
        // `error_*` stop_reason subtypes are themselves error terminations,
        // even with no accumulated error message.
        let stop_reason_is_error = stop_reason.starts_with("error_");
        let is_error = qr.cancelled
            || qr.budget_exhausted
            || stop_reason_is_error
            || !error_messages.is_empty();
        let mut errors = error_messages;
        if let Some(payload) = qr.max_turns_reached.as_ref() {
            errors.push(format!(
                "Reached maximum number of turns ({})",
                payload.max_turns
            ));
        }
        if stop_reason == "error_max_structured_output_retries" {
            let cap = crate::config::max_structured_output_retries();
            errors.push(format!(
                "Failed to provide valid structured output after {cap} attempts"
            ));
        }

        coco_types::SessionResultParams {
            session_id: self.config.session_id.clone(),
            total_turns: qr.turns,
            duration_ms: qr.duration_ms,
            duration_api_ms: qr.duration_api_ms,
            is_error,
            stop_reason,
            total_cost_usd: qr.cost_tracker.total_cost_usd(),
            usage: qr.total_usage,
            model_usage,
            // Accumulated across PermissionDecision::Deny branches.
            permission_denials: qr.permission_denials.clone(),
            result: if is_error {
                None
            } else {
                Some(qr.response_text.clone())
            },
            errors,
            structured_output: if is_error {
                None
            } else {
                qr.structured_output.clone()
            },
            fast_mode_state: None,
            num_api_calls: Some(qr.cost_tracker.total_api_calls as i32),
        }
    }

    /// Build an orchestration context from the engine's config.
    pub(crate) fn orchestration_ctx(&self) -> OrchestrationContext {
        OrchestrationContext {
            session_id: self.config.session_id.clone(),
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: self.config.project_dir.clone(),
            permission_mode: Some(format!("{:?}", self.config.permission_mode)),
            // Main-thread orchestration: no subagent identity. Per-spawn
            // contexts are constructed in `coco-coordinator` via
            // `hook_ctx_for_subagent`.
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: self.config.disable_all_hooks,
            allow_managed_hooks_only: self.config.allow_managed_hooks_only,
            attachment_emitter: self.attachment_emitter(),
            sync_event_sink: self.sync_hook_buffer.clone(),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: self.async_hook_registry.clone(),
            async_rewake_sink: None,
            llm_handle: self.hook_llm_handle.clone(),
            workspace_trust_accepted: None,
        }
    }

    /// Consume `HookExecutionEvent` from the orchestration layer and forward
    /// them as `CoreEvent::Protocol(HookStarted/Progress/Response)`.
    ///
    /// Associated function (no `&self`) so callers can drive a standalone
    /// task with `tokio::spawn(QueryEngine::forward_hook_events(...))`. Tests
    /// rely on this calling convention.
    ///
    /// Graceful shutdown: the normal exit path is for the caller to drop
    /// the matching sender, which makes `rx.recv()` return `None` and
    /// drains any queued events before returning. The `cancel` token is
    /// a fast-path escape hatch for crash scenarios (e.g. the drain
    /// timeout in `run_internal_with_messages` has expired); when
    /// cancelled, pending events are discarded. See plan file WS-5.
    pub(crate) async fn forward_hook_events(
        mut rx: tokio::sync::mpsc::Receiver<coco_hooks::HookExecutionEvent>,
        core_tx: Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        cancel: CancellationToken,
    ) {
        let Some(core_tx) = core_tx else {
            return;
        };
        loop {
            let evt = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                maybe = rx.recv() => match maybe {
                    Some(evt) => evt,
                    None => break,
                },
            };
            let notif = match evt {
                coco_hooks::HookExecutionEvent::Started {
                    hook_id,
                    hook_name,
                    hook_event,
                } => ServerNotification::HookStarted(coco_types::HookStartedParams {
                    hook_id,
                    hook_name,
                    hook_event,
                }),
                coco_hooks::HookExecutionEvent::Progress {
                    hook_id,
                    hook_name,
                    stdout,
                    stderr,
                } => ServerNotification::HookProgress(coco_types::HookProgressParams {
                    hook_id,
                    hook_name,
                    // The orchestration-layer event doesn't carry the
                    // hook event name on Progress; consumers can correlate
                    // via `hook_id` against the preceding Started event.
                    hook_event: String::new(),
                    stdout,
                    stderr,
                    output: String::new(),
                }),
                coco_hooks::HookExecutionEvent::Response {
                    hook_id,
                    hook_name,
                    exit_code,
                    stdout,
                    stderr,
                    outcome,
                } => ServerNotification::HookResponse(coco_types::HookResponseParams {
                    hook_id,
                    hook_name,
                    hook_event: String::new(),
                    // orchestration layer merges stdout into output on
                    // the raw event; expose both fields separately for
                    // SDK consumers.
                    output: stdout.clone(),
                    stdout,
                    stderr,
                    exit_code,
                    outcome: hook_outcome_to_status(outcome),
                }),
            };
            if !emit_protocol_owned(&core_tx, notif).await {
                break;
            }
        }
    }
}
