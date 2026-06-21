//! `initialize` + full session lifecycle (`session/*`) + per-turn event
//! forwarding and session-stat aggregation.

use std::sync::Arc;

use coco_types::CoreEvent;
use coco_types::InitializeResult;
use coco_types::SdkAccountInfo;
use coco_types::SdkModelInfo;
use coco_types::SessionStartResult;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::DEFAULT_SDK_FAST_MODEL;
use super::DEFAULT_SDK_MODEL;
use super::HandlerContext;
use super::HandlerResult;
use super::PROTOCOL_VERSION;
use super::SdkServerState;
use super::SessionHandle;
use crate::sdk_server::outbound::OutboundMessage;

/// `initialize` — capability negotiation. Returns an `InitializeResult`.
///
/// Data sourcing:
/// - `models`: static list of the two Anthropic models coco-rs ships with
///   (promoted from a fixed table; model discovery is a separate follow-up).
/// - `commands`, `agents`, `account`, `output_style`,
///   `available_output_styles`, `fast_mode_state`: populated from an
///   optional [`super::InitializeBootstrap`] trait object installed via
///   `SdkServer::with_initialize_bootstrap()`. When no bootstrap is
///   wired the handler returns defaults (empty lists / default account /
///   `"default"` output style).
/// - Internal `_cocoRs*` extension fields carry the coco-rs binary and
///   protocol version for debugging.
pub(super) async fn handle_initialize(
    params: coco_types::InitializeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    info!("SdkServer: initialize");

    // Stores the agentProgressSummaries flag on `SdkServerState` so
    // subsequent `session/start` calls can copy it onto the new
    // session's `ToolAppState`. Default-off is intentional: a fully
    // saturated coordinator burns up to 32 side-query LLM calls per
    // minute on summarization, so opt-in semantics keep that off the
    // user's hot path.
    if params.agent_progress_summaries.unwrap_or(false) {
        ctx.state
            .agent_progress_summaries_enabled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        info!("SdkServer: agentProgressSummaries enabled by client");
    }

    if let Some(runtime) = ctx.state.session_runtime.read().await.clone()
        && let Some(hooks) = params.hooks.as_ref()
    {
        crate::sdk_server::sdk_hooks::install_runtime_callback(ctx.state.clone(), &runtime);
        let registered = crate::sdk_server::sdk_hooks::register_initialize_hooks(&runtime, hooks);
        info!(
            target: "coco::sdk_server::initialize",
            registered,
            "SDK hook callbacks registered"
        );
    }

    if let Some(server_names) = params.sdk_mcp_servers.clone()
        && !server_names.is_empty()
    {
        let state = ctx.state.clone();
        tokio::spawn(async move {
            if let Err(error) =
                crate::sdk_server::sdk_mcp::register_and_connect(state, server_names).await
            {
                warn!(error = %error, "SDK MCP registration failed");
            }
        });
    }

    // When the SDK client pushes `initialize.agents`, parse the JSON map
    // into `AgentDefinition` entries (tagged `AgentSource::FlagSettings`)
    // and stash them on `SdkServerState` so:
    //   - the `agents()` listing below merges them into the response;
    //   - `session/start` drains the stash into the new session's
    //     `AgentDefinitionStore`.
    //
    // Parse failures don't fail the initialize handshake — log and
    // continue with the accepted subset.
    if let Some(agents_map) = params.agents.as_ref() {
        let (accepted, errors) =
            crate::sdk_server::cli_bootstrap::parse_sdk_agent_definitions(agents_map);
        if !errors.is_empty() {
            for err in &errors {
                tracing::warn!(target: "coco::sdk_server::initialize", "SDK agent parse error: {err}");
            }
        }
        let accepted_count = accepted.len();
        if accepted_count > 0 {
            // Stash on SdkServerState first (so the agents()
            // listing below sees them in this initialize response,
            // even when the SessionRuntime isn't wired yet — e.g.
            // in tests).
            {
                let mut stash = ctx.state.pending_sdk_agents.write().await;
                // Replace (not extend) — `initialize` is a fresh handshake;
                // a prior connection's stash should not bleed into this one.
                *stash = accepted.clone();
            }
            // Inject into the live SessionRuntime so subsequent
            // `turn/start` spawns can actually use the SDK-supplied agents.
            // The SessionRuntime is None in tests; the stash-only
            // branch above still gives the wire response correct
            // contents.
            if let Some(runtime) = ctx.state.session_runtime.read().await.clone() {
                runtime.set_sdk_supplied_agents(accepted).await;
                info!(
                    target: "coco::sdk_server::initialize",
                    count = accepted_count,
                    "SDK-supplied agents injected into SessionRuntime catalog"
                );
            } else {
                info!(
                    target: "coco::sdk_server::initialize",
                    count = accepted_count,
                    "SDK-supplied agents stashed (no SessionRuntime yet)"
                );
            }
        }
    }

    // Pull the bootstrap provider out of state, drop the read guard, then
    // call its async accessors. Holding the guard across awaits would
    // block any concurrent mutation (e.g. a hot-swap via builder).
    let bootstrap = {
        let slot = ctx.state.initialize_bootstrap.read().await;
        slot.as_ref().map(Arc::clone)
    };

    let (commands, mut agents, account, output_style, available_output_styles, fast_mode_state) =
        if let Some(b) = bootstrap {
            (
                b.commands().await,
                b.agents().await,
                b.account().await,
                b.output_style().await,
                b.available_output_styles().await,
                b.fast_mode_state().await,
            )
        } else {
            (
                Vec::new(),
                Vec::new(),
                SdkAccountInfo::default(),
                "default".into(),
                vec!["default".into()],
                None,
            )
        };

    // Merge SDK-supplied agents into the response listing so the client
    // immediately sees what it pushed. Stashed entries always win —
    // they're the freshest user intent.
    {
        let stash = ctx.state.pending_sdk_agents.read().await;
        if !stash.is_empty() {
            let stash_names: std::collections::HashSet<String> =
                stash.iter().map(|d| d.agent_type.to_string()).collect();
            agents.retain(|a| !stash_names.contains(&a.name));
            agents.extend(stash.iter().cloned().map(|d| coco_types::SdkAgentInfo {
                name: d.name,
                description: d.description.unwrap_or_default(),
                model: d.model,
            }));
            agents.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    let result = InitializeResult {
        commands,
        agents,
        output_style,
        available_output_styles,
        models: vec![
            SdkModelInfo {
                value: DEFAULT_SDK_MODEL.into(),
                display_name: "Claude Opus 4.6".into(),
                description: "Anthropic's most capable model for deep reasoning tasks.".into(),
                supports_effort: Some(true),
                supported_effort_levels: Vec::new(),
                supports_adaptive_thinking: Some(true),
                supports_fast_mode: Some(true),
                supports_auto_mode: Some(true),
            },
            SdkModelInfo {
                value: DEFAULT_SDK_FAST_MODEL.into(),
                display_name: "Claude Sonnet 4.6".into(),
                description: "Fast, cost-efficient model for everyday coding tasks.".into(),
                supports_effort: Some(true),
                supported_effort_levels: Vec::new(),
                supports_adaptive_thinking: Some(true),
                supports_fast_mode: Some(true),
                supports_auto_mode: Some(true),
            },
        ],
        account,
        pid: Some(std::process::id()),
        fast_mode_state,
        protocol_version: PROTOCOL_VERSION.into(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    HandlerResult::ok(result)
}

/// `session/start` — create a new SDK session.
///
/// Records the session in `SdkServerState.session` and returns a generated
/// `session_id`. The QueryEngine is not spawned here — `turn/start` does
/// that per-turn.
pub(super) async fn handle_session_start(
    params: coco_types::SessionStartParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Short critical section: check the slot is empty, then drop the lock.
    // Holding the session write lock across the subsequent disk persistence
    // would stall every other session-touching handler (turn/start,
    // turn/interrupt, session/archive, and the event forwarder's
    // stat-accumulation path) for the duration of the fs write.
    {
        let session_slot = ctx.state.session.read().await;
        if session_slot.is_some() {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "a session is already active; archive it first or use session/resume"
                    .into(),
                data: None,
            };
        }
    }

    let session_id = format!("session-{}", uuid::Uuid::new_v4());
    let cwd = params.cwd.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let model = params
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_SDK_MODEL.into());

    info!(session_id = %session_id, cwd = %cwd, model = %model, "SdkServer: session/start");

    // Persist to disk if a SessionManager is wired. This makes the
    // session visible to `session/list` and resumable via
    // `session/resume`. Failure to persist is non-fatal — the session
    // still runs in-memory; we log a warning and continue.
    //
    // `SessionManager::save` is sync (`std::fs::write`); run it on the
    // blocking pool so the tokio worker isn't stalled by disk I/O.
    // The manager Arc is cloned out of the read guard which is then
    // dropped BEFORE the spawn_blocking await — holding the guard
    // across a blocking call would serialize every session_manager
    // reader behind this request.
    let manager_arc = {
        let manager_slot = ctx.state.session_manager.read().await;
        manager_slot.as_ref().map(Arc::clone)
    };
    if let Some(manager) = manager_arc {
        let record = coco_session::Session {
            id: session_id.clone(),
            created_at: coco_session::timestamp_now(),
            updated_at: None,
            model: model.clone(),
            working_dir: std::path::PathBuf::from(&cwd),
            title: None,
            message_count: 0,
            total_tokens: 0,
            tags: Vec::new(),
        };
        let save_result = tokio::task::spawn_blocking(move || manager.save(&record)).await;
        match save_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!(
                session_id = %session_id,
                error = %e,
                "session/start: failed to persist session to disk"
            ),
            Err(join_err) => warn!(
                session_id = %session_id,
                error = %join_err,
                "session/start: persistence task panicked"
            ),
        }
    }

    // Re-acquire the write lock and install the session. A concurrent
    // session/start would have hit the `is_some()` guard above and
    // errored out before persistence, so this path only races with an
    // external archive — in which case we claim the slot first and
    // the archiver moves on.
    let mut session_slot = ctx.state.session.write().await;
    if session_slot.is_some() {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "a session was started concurrently; retry with session/archive first".into(),
            data: None,
        };
    }
    let mut handle = SessionHandle::new(session_id.clone(), cwd, model);
    if let Some(mode) = params.permission_mode {
        handle.permission_mode = Some(mode);
        // Brand-new session: the engine config / rules aren't wired yet, so the
        // Auto-entry stash starts empty. The evaluator-facing strip in
        // ToolContextFactory::build (keyed on live mode==Auto) is the real guard.
        crate::live_permission_mode::apply_to_app_state(
            &handle.app_state,
            coco_types::PermissionMode::Default,
            mode,
            &coco_types::PermissionRulesBySource::new(),
        )
        .await;
    }
    // Copy the SDK-level agentProgressSummaries flag onto the new
    // session's ToolAppState so the bg AgentTool path can gate
    // periodic-summary timers without reaching into SdkServerState.
    // Coordinator mode auto-enables independently.
    if ctx
        .state
        .agent_progress_summaries_enabled
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        handle
            .app_state
            .write()
            .await
            .agent_progress_summaries_enabled = true;
    }
    *session_slot = Some(handle);
    drop(session_slot);

    // Refresh per-session subsystems on the shared `SessionRuntime` so
    // sequential `session/archive` → `session/start` cycles don't leak
    // the prior session's `FileReadState` LRU, `SessionMemoryService`
    // disk path, file-history sink session id, or cache-break detector
    // baseline into the new session.
    //
    // `runtime.start_new_session` also retargets the runtime-owned
    // `TranscriptFileHistorySink` (when file-checkpointing is enabled)
    // via its shared `Arc<RwLock<String>>` session_id, so file-history
    // snapshots written from the engine land in the new session's jsonl.
    let runtime_arc = {
        let slot = ctx.state.session_runtime.read().await;
        slot.as_ref().cloned()
    };
    if let Some(runtime) = runtime_arc {
        runtime.start_new_session(session_id.clone()).await;
    }

    HandlerResult::ok(SessionStartResult { session_id })
}

/// Drain per-turn CoreEvents and forward to the outbound notification
/// channel, intercepting session envelope events.
///
/// Specifically:
/// - `SessionResult` events are **not** forwarded. Instead, their stats
///   are folded into `SessionHandle.stats` (only if the current session
///   still matches `owner_session_id`). The aggregated `SessionResult`
///   is emitted once when `session/archive` runs.
/// - `SessionStarted` events are also swallowed (defensive — the current
///   runner doesn't emit them, but if a future runner enables the
///   bootstrap path, we still want exactly one per session from the SDK
///   server side, not one per turn).
/// - All other events pass through unchanged.
///
/// `owner_session_id` is the session this forwarder was created for.
/// If `session/archive` + `session/start` has replaced the active
/// session while this forwarder is still draining, we refuse to fold
/// stats into the unrelated new session.
pub(super) async fn forward_turn_events(
    mut rx: mpsc::Receiver<CoreEvent>,
    tx: mpsc::Sender<OutboundMessage>,
    state: Arc<SdkServerState>,
    owner_session_id: String,
) {
    use coco_types::ServerNotification;
    while let Some(event) = rx.recv().await {
        match event {
            CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
                accumulate_session_result(&state, &owner_session_id, &params).await;
                // Swallow — aggregated result is emitted by session/archive.
            }
            CoreEvent::Protocol(ServerNotification::SessionStarted(_)) => {
                // Swallow: SessionStarted is owned by the SDK server, not the engine.
            }
            other => {
                if tx.send(OutboundMessage::core_event(other)).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Fold a per-turn `SessionResult` into the active session's aggregated
/// stats. No-op if:
/// - the session has already been archived, OR
/// - the current session's id doesn't match `owner_session_id` (the
///   turn belongs to an already-archived session whose cleanup is
///   still winding down; we must not contaminate the successor session)
async fn accumulate_session_result(
    state: &SdkServerState,
    owner_session_id: &str,
    params: &coco_types::SessionResultParams,
) {
    let mut slot = state.session.write().await;
    let Some(session) = slot.as_mut() else {
        return;
    };
    if session.session_id != owner_session_id {
        // Cross-session guard: the slot now holds a different session.
        // This turn's stats belong to a dead session — drop them.
        debug!(
            owner = owner_session_id,
            current = %session.session_id,
            "accumulate_session_result: session mismatch, dropping stats"
        );
        return;
    }
    let s = &mut session.stats;
    s.total_turns = s.total_turns.saturating_add(1);
    s.total_duration_api_ms = s
        .total_duration_api_ms
        .saturating_add(params.duration_api_ms);
    s.total_cost_usd += params.total_cost_usd;
    s.usage += params.usage;
    for (model, mu) in &params.model_usage {
        let entry = s.model_usage.entry(model.clone()).or_default();
        entry.input_tokens = entry.input_tokens.saturating_add(mu.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(mu.output_tokens);
        entry.cache_read_input_tokens = entry
            .cache_read_input_tokens
            .saturating_add(mu.cache_read_input_tokens);
        entry.cache_creation_input_tokens = entry
            .cache_creation_input_tokens
            .saturating_add(mu.cache_creation_input_tokens);
        entry.web_search_requests = entry
            .web_search_requests
            .saturating_add(mu.web_search_requests);
        entry.cost_usd += mu.cost_usd;
    }
    s.permission_denials
        .extend(params.permission_denials.iter().cloned());
    if params.result.is_some() {
        s.last_result_text = params.result.clone();
    }
    if params.structured_output.is_some() {
        s.structured_output = params.structured_output.clone();
    }
    s.last_stop_reason = Some(params.stop_reason.clone());
    if params.is_error {
        s.had_error = true;
        s.errors.extend(params.errors.iter().cloned());
    }
    if let Some(n) = params.num_api_calls {
        s.num_api_calls = s.num_api_calls.saturating_add(n);
    }
}

/// `session/archive` — drop the active session slot.
///
/// Emits the aggregated `SessionResult` (built from the session's
/// accumulated stats) as a final notification before clearing the slot.
/// This gives SDK clients exactly one `SessionResult` per session,
/// regardless of how many `turn/start` calls happened inside it.
///
/// **Ordering note**: The `SessionResult` notification and archive
/// response both go through the dispatcher's ordered outbound queue, so
/// the client sees the aggregate before the JSON-RPC response.
///
/// **Archive-during-running-turn**: If a turn is in flight when
/// `session/archive` is called, the aggregate is built from whatever
/// stats have been accumulated so far (the in-flight turn's stats are
/// NOT included — it's cancelled after the aggregate is built). This
/// Archive discards in-progress work.
///
/// Errors:
/// - `INVALID_REQUEST` if no session is active
/// - `INVALID_REQUEST` if the `session_id` param doesn't match the
///   currently-active session (prevents clients from archiving someone
///   else's session by mistake)
pub(super) async fn handle_session_archive(
    params: coco_types::SessionArchiveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Hold the write lock for the entire archive operation:
    // (1) validate session_id, (2) build the aggregate from a
    // consistent snapshot, (3) clear the slot. This closes the
    // TOCTOU window that an earlier read/write-lock split opened —
    // a concurrent `forward_turn_events` forwarder cannot slip a
    // `SessionResult` into stats between the aggregate build and
    // the clear, because it contends for the same write lock and we
    // hold it end-to-end here.
    //
    // Cancellation of an in-flight turn and emission of the
    // aggregated notification both happen AFTER the lock is released:
    // cancellation is idempotent and the notification send doesn't
    // need the session lock.
    let (result_params, token_to_cancel, turn_handle, forwarder_handle) = {
        let mut slot = ctx.state.session.write().await;
        let Some(session) = slot.as_mut() else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session to archive".into(),
                data: None,
            };
        };
        if session.session_id != params.session_id {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!(
                    "session_id mismatch: active is {}, archive requested for {}",
                    session.session_id, params.session_id
                ),
                data: None,
            };
        }
        info!(session_id = %params.session_id, "SdkServer: session/archive");
        let result = build_aggregated_session_result(session);
        // Take ownership of the cancel token and both JoinHandles before
        // clearing the slot. The turn task's normal-completion cleanup
        // path checks `session.session_id` against its `owner_session_id`
        // and no-ops if the slot is cleared — so taking these out here
        // is safe w.r.t. the concurrent task cleanup.
        let token = session.active_turn_cancel.take();
        let turn_handle = session.active_turn_task.take();
        let forwarder_handle = session.active_turn_forwarder.take();
        // Clear the slot under the write lock so any racing forwarder
        // that later acquires the lock sees `None` and no-ops.
        *slot = None;
        (result, token, turn_handle, forwarder_handle)
    };

    // Cancel any running turn. Outside the lock because:
    //   (a) `CancellationToken::cancel` is cheap and non-blocking
    //   (b) the turn task's subsequent cleanup (writing to session
    //       slot to clear `active_turn_cancel`) also takes the write
    //       lock — holding it here would deadlock
    if let Some(token) = token_to_cancel {
        token.cancel();
    }

    // Drain the in-flight turn before emitting the aggregated result.
    //
    // Ordering contract: the client must see every per-turn event
    // BEFORE the aggregated `SessionResult` for the session, otherwise
    // a late `AgentMessageDelta` / `TurnFailed` slipping out after the
    // archive notification confuses the wire stream.
    //
    // Sequence is:
    //   1. Wait for the runner task to exit (it drops its `inner_tx`).
    //   2. Wait for the forwarder task to exit (it sees channel closed
    //      once `inner_tx` is dropped, drains any buffered events, and
    //      returns from its loop).
    //
    // Both awaits are bounded by a 5s timeout so a pathological runner
    // ignoring the cancel token can't hang archive indefinitely.
    if let Some(handle) = turn_handle {
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %params.session_id,
                error = %join_err,
                "session/archive: turn task join failed"
            ),
            Err(_) => warn!(
                session_id = %params.session_id,
                "session/archive: turn task did not exit within 5s of cancel; \
                 emitting aggregate anyway (late events may still follow)"
            ),
        }
    }
    if let Some(handle) = forwarder_handle {
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %params.session_id,
                error = %join_err,
                "session/archive: forwarder task join failed"
            ),
            Err(_) => warn!(
                session_id = %params.session_id,
                "session/archive: forwarder task did not drain within 5s"
            ),
        }
    }

    // Delete the persisted session record if a SessionManager is wired.
    // Non-fatal — log and continue if disk delete fails. Runs on the
    // blocking pool to avoid stalling the tokio worker on `remove_file`.
    // Clone the Arc out and drop the read guard before the blocking
    // call so other readers aren't serialized behind disk I/O.
    let manager_arc = {
        let manager_slot = ctx.state.session_manager.read().await;
        manager_slot.as_ref().map(Arc::clone)
    };
    if let Some(manager) = manager_arc {
        let target_id = params.session_id.clone();
        let delete_result = tokio::task::spawn_blocking(move || manager.delete(&target_id)).await;
        match delete_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!(
                session_id = %params.session_id,
                error = %e,
                "session/archive: failed to delete persisted session record"
            ),
            Err(join_err) => warn!(
                session_id = %params.session_id,
                error = %join_err,
                "session/archive: delete task panicked"
            ),
        }
    }

    // Emit the aggregated SessionResult on the outbound notification
    // channel. Ignore a send error (transport may have shut down)
    // since the state is already cleared.
    let result_event = CoreEvent::Protocol(coco_types::ServerNotification::SessionResult(
        Box::new(result_params),
    ));
    let _ = ctx
        .notif_tx
        .send(OutboundMessage::core_event(result_event))
        .await;

    HandlerResult::ok_empty()
}

/// Build a final `SessionResultParams` from an active session's
/// accumulated stats. Used by `session/archive` to synthesize the
/// once-per-session aggregate the SDK client expects.
fn build_aggregated_session_result(session: &SessionHandle) -> coco_types::SessionResultParams {
    let stats = &session.stats;
    coco_types::SessionResultParams {
        session_id: session.session_id.clone(),
        total_turns: stats.total_turns,
        duration_ms: session.started_at.elapsed().as_millis() as i64,
        duration_api_ms: stats.total_duration_api_ms,
        is_error: stats.had_error,
        stop_reason: stats
            .last_stop_reason
            .clone()
            .unwrap_or_else(|| "archived".into()),
        total_cost_usd: stats.total_cost_usd,
        usage: stats.usage,
        model_usage: stats.model_usage.clone(),
        permission_denials: stats.permission_denials.clone(),
        result: stats.last_result_text.clone(),
        errors: stats.errors.clone(),
        structured_output: if stats.had_error {
            None
        } else {
            stats.structured_output.clone()
        },
        fast_mode_state: None,
        num_api_calls: if stats.num_api_calls > 0 {
            Some(stats.num_api_calls)
        } else {
            None
        },
    }
}

/// Convert a `coco_session::Session` record to the wire-format
/// summary used by list/read/resume results.
fn session_to_summary(s: &coco_session::Session) -> coco_types::SdkSessionSummary {
    coco_types::SdkSessionSummary {
        session_id: s.id.clone(),
        model: s.model.clone(),
        cwd: s.working_dir.to_string_lossy().into_owned(),
        created_at: s.created_at.clone(),
        updated_at: s.updated_at.clone(),
        title: s.title.clone(),
        message_count: s.message_count,
        total_tokens: s.total_tokens,
    }
}

/// `session/list` — enumerate persisted sessions, newest first.
///
/// Delegates to `SessionManager::list()`. Returns an empty list if no
/// manager is wired (session persistence disabled).
///
/// Errors:
/// - `INTERNAL_ERROR` if `SessionManager::list()` fails (e.g. filesystem error)
pub(super) async fn handle_session_list(ctx: &HandlerContext) -> HandlerResult {
    // `list()` walks the session directory with `read_dir` and reads every
    // JSON blob synchronously — offload to the blocking pool so a session-
    // browser client polling this endpoint can't stall the tokio worker.
    // Clone the Arc out and drop the read guard before the blocking call.
    let manager = {
        let slot = ctx.state.session_manager.read().await;
        match slot.as_ref() {
            Some(m) => Arc::clone(m),
            None => {
                info!("SdkServer: session/list (no session manager installed, returning empty)");
                return HandlerResult::ok(coco_types::SessionListResult::default());
            }
        }
    };
    let list_result = tokio::task::spawn_blocking(move || manager.list()).await;
    match list_result {
        Ok(Ok(sessions)) => {
            let summaries = sessions.iter().map(session_to_summary).collect::<Vec<_>>();
            info!(count = summaries.len(), "SdkServer: session/list");
            HandlerResult::ok(coco_types::SessionListResult {
                sessions: summaries,
            })
        }
        Ok(Err(e)) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("session/list failed: {e}"),
            data: None,
        },
        Err(join_err) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("session/list task panicked: {join_err}"),
            data: None,
        },
    }
}

/// `session/read` — load a single persisted session's metadata.
///
/// Returns the summary only; message history retrieval via the JSONL
/// transcript is a follow-up.
///
/// Errors:
/// - `INVALID_REQUEST` if no session manager is wired
/// - `INVALID_REQUEST` if the session_id is not found on disk
pub(super) async fn handle_session_read(
    params: coco_types::SessionReadParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Clone the Arc out and drop the read guard before the blocking call.
    let manager = {
        let slot = ctx.state.session_manager.read().await;
        match slot.as_ref() {
            Some(m) => Arc::clone(m),
            None => {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "session persistence is not enabled on this server".into(),
                    data: None,
                };
            }
        }
    };
    let session_id = params.session_id.clone();
    let load_result = tokio::task::spawn_blocking(move || manager.load(&session_id)).await;
    match load_result {
        Ok(Ok(session)) => {
            info!(session_id = %params.session_id, "SdkServer: session/read");
            HandlerResult::ok(coco_types::SessionReadResult {
                session: session_to_summary(&session),
                messages: Vec::new(),
                next_cursor: None,
                has_more: false,
            })
        }
        Ok(Err(e)) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("session/read: {e}"),
            data: None,
        },
        Err(join_err) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("session/read task panicked: {join_err}"),
            data: None,
        },
    }
}

/// `session/resume` — load a persisted session from disk and install
/// it as the active session, including the JSONL message history so
/// the next turn the SDK client drives sees the prior chain.
///
/// Replaces the current session slot (if any) with a fresh
/// `SessionHandle` built from the persisted metadata. Any in-flight
/// turn on the previous session is cancelled first to prevent
/// orphaned state. When a `SessionRuntime` is wired, the runtime's
/// session id is repointed and `runtime.history` is seeded with the
/// loaded messages; the transcript dedup set is pre-populated so the
/// per-turn JSONL append doesn't re-write entries already on disk.
///
/// Errors:
/// - `INVALID_REQUEST` if no session manager is wired
/// - `INVALID_REQUEST` if the session_id is not found on disk
/// - `INTERNAL_ERROR` if the session manager's resume operation fails
///   or the JSONL transcript fails to load
pub(super) async fn handle_session_resume(
    params: coco_types::SessionResumeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let manager_slot = ctx.state.session_manager.read().await;
    let Some(manager) = manager_slot.as_ref() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session persistence is not enabled on this server".into(),
            data: None,
        };
    };
    let manager_arc = Arc::clone(manager);
    // Release the manager read lock before acquiring the session write lock
    // to avoid potential lock-ordering complications in future refactors.
    drop(manager_slot);
    let target_id = params.session_id.clone();
    let resume_result = tokio::task::spawn_blocking(move || manager_arc.resume(&target_id)).await;
    let session = match resume_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!("session/resume: {e}"),
                data: None,
            };
        }
        Err(join_err) => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("session/resume task panicked: {join_err}"),
                data: None,
            };
        }
    };

    // Load the JSONL transcript so the resumed run sees its own
    // history. Best-effort: a missing transcript leaves the runtime
    // with an empty history — the resumed session starts cold rather
    // than failing the resume call.
    //
    // The transcript lives in the resumed session's own project tree
    // (`<memory_base>/projects/<slug>/<sid>.jsonl`). Route through
    // `resolve_session_file_path` so a linked worktree (whose long cwd
    // path produces a different djb2 slug suffix than its sibling repo)
    // still resolves to the right file.
    let memory_base = coco_config::global_config::config_home();
    let transcript_path = coco_session::storage::resolve_session_file_path(
        &memory_base,
        &session.id,
        Some(&session.working_dir),
    )
    .ok()
    .flatten()
    .map(|r| r.file_path);
    let conversation = if let Some(transcript_path) = transcript_path.as_ref() {
        match coco_session::recovery::load_conversation_for_resume(transcript_path) {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(
                    session_id = %session.id,
                    error = %e,
                    "session/resume: transcript load failed; resuming with empty history"
                );
                None
            }
        }
    } else {
        None
    };

    // Install as the active session. If a session is already active,
    // cancel any in-flight turn and replace it — `session/resume`
    // implicitly archives the prior session.
    let mut slot = ctx.state.session.write().await;
    if let Some(prior) = slot.as_ref()
        && let Some(token) = &prior.active_turn_cancel
    {
        warn!(
            prior_session = %prior.session_id,
            new_session = %session.id,
            "SdkServer: session/resume replaces active session; cancelling in-flight turn"
        );
        token.cancel();
    }
    *slot = Some(SessionHandle::new(
        session.id.clone(),
        session.working_dir.to_string_lossy().into_owned(),
        session.model.clone(),
    ));
    drop(slot);

    // When a `SessionRuntime` is wired (the production path), repoint
    // its session id at the resumed target and seed the in-memory
    // history + transcript dedup so the next turn's prompt build sees
    // the loaded chain rather than starting cold.
    if let Some(runtime) = ctx.state.session_runtime.read().await.clone() {
        runtime.start_new_session(session.id.clone()).await;
        if let Some(conversation) = &conversation {
            {
                let mut h = runtime.history.lock().await;
                h.clear();
                for m in conversation.messages.iter().cloned() {
                    h.push(m);
                }
            }
            runtime
                .seed_transcript_dedup(
                    conversation
                        .messages
                        .iter()
                        .filter_map(|m| m.uuid().copied()),
                )
                .await;
            // SDK `session/resume` is main-thread only — subagent
            // resume goes through the AgentTool spawn API. agent_id =
            // None matches the load filter for main-thread records.
            runtime
                .seed_tool_result_replacement_state(&conversation.messages, &session.id, None)
                .await;
        }
    }

    info!(
        session_id = %session.id,
        prior_messages = conversation
            .as_ref()
            .map(|conversation| conversation.messages.len())
            .unwrap_or(0),
        "SdkServer: session/resume"
    );
    HandlerResult::ok(coco_types::SessionResumeResult {
        session: session_to_summary(&session),
    })
}
