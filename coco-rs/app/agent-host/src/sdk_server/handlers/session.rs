//! `initialize` + full session lifecycle (`session/*`) + per-turn event
//! forwarding and session-stat aggregation.

use std::sync::Arc;

use coco_types::{
    CoreEvent, InitializeResult, SdkAccountInfo, SdkAgentInfo, SdkModelInfo, SdkSlashCommand,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{
    DEFAULT_SDK_FAST_MODEL, DEFAULT_SDK_MODEL, HandlerContext, HandlerResult, PROTOCOL_VERSION,
    SdkServerState,
};
use crate::{
    headless::build_output_style_manager,
    sdk_server::{
        cli_bootstrap::def_to_sdk_agent_info,
        outbound::{OutboundMessage, send_session_event, send_session_event_and_wait},
        session_data,
        session_data::PersistedSessionDataError,
    },
    session_runtime::SessionHandle,
};

pub(crate) struct LoadedResumeSession {
    pub session: coco_session::Session,
    pub session_id: coco_types::SessionId,
    pub conversation: coco_session::recovery::ConversationForResume,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedStartSession {
    pub session_id: coco_types::SessionId,
    pub cwd: String,
    pub model: String,
    pub permission_mode: Option<coco_types::PermissionMode>,
    pub agent_progress_summaries_enabled: bool,
}

/// `initialize` — capability negotiation. Returns an `InitializeResult`.
///
/// Data sourcing:
/// - `models`: static list of the two Anthropic models coco-rs ships with
///  (promoted from a fixed table; model discovery is a separate follow-up).
/// - `commands`, `agents`, `output_style`, `available_output_styles`:
///   populated from the live [`SessionHandle`] when installed, so SDK
///   initialize reflects the active runtime after replacements. Before a
///   runtime exists, these fall back to the optional
///   [`super::InitializeBootstrap`] snapshot installed via
///   `SdkServer::with_initialize_bootstrap()`.
/// - `fast_mode_state`: populated from the live [`SessionHandle`] when
///   installed, falling back to the optional bootstrap provider before runtime
///   construction.
/// - `account`: populated from the optional bootstrap provider until auth
///   sources grow runtime-owned accessors.
/// - Internal `_cocoRs*` extension fields carry the coco-rs binary and
///   protocol version for debugging.
pub(super) async fn handle_initialize(
    _params: coco_types::InitializeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    info!("SdkServer: initialize");
    let params = ctx.connection_profile.initialize();

    // When the SDK client pushes `initialize.agents`, parse the JSON map
    // into `AgentDefinition` entries (tagged `AgentSource::FlagSettings`)
    // and stash them on `SdkServerState` so:
    //   - the `agents()` listing below merges them into the response;
    //   - `session/start` drains the stash into the new session's
    //     `AgentDefinitionStore`.
    //
    // Parse failures don't fail the initialize handshake — log and
    // continue with the accepted subset.
    let accepted_sdk_agents = if let Some(agents_map) = params.agents.as_ref() {
        let (accepted, errors) =
            crate::sdk_server::cli_bootstrap::parse_sdk_agent_definitions(agents_map);
        if !errors.is_empty() {
            for err in &errors {
                tracing::warn!(target: "coco::sdk_server::initialize", "SDK agent parse error: {err}");
            }
        }
        accepted
    } else {
        Vec::new()
    };

    // Pull the bootstrap provider out of state, drop the read guard, then
    // call its async accessors. Holding the guard across awaits would
    // block any concurrent mutation (e.g. a hot-swap via builder).
    let bootstrap = ctx.state.initialize_bootstrap_snapshot().await;
    let runtime = ctx.resolve_runtime().await;

    let (commands, mut agents, output_style, available_output_styles) =
        if let Some(runtime) = runtime.as_ref() {
            runtime_initialize_metadata(runtime).await
        } else if let Some(b) = bootstrap.as_ref() {
            (
                b.commands().await,
                b.agents().await,
                b.output_style().await,
                b.available_output_styles().await,
            )
        } else {
            (
                Vec::new(),
                Vec::new(),
                "default".into(),
                vec!["default".into()],
            )
        };

    let account = if let Some(b) = bootstrap.as_ref() {
        b.account().await
    } else {
        SdkAccountInfo::default()
    };
    let fast_mode_state = if let Some(runtime) = runtime.as_ref() {
        Some(runtime_fast_mode_state(runtime).await)
    } else if let Some(b) = bootstrap.as_ref() {
        b.fast_mode_state().await
    } else {
        None
    };

    // Merge SDK-supplied agents into the response listing so the client
    // immediately sees what it pushed. Stashed entries always win —
    // they're the freshest user intent.
    {
        let stash = accepted_sdk_agents;
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

async fn runtime_initialize_metadata(
    runtime: &SessionHandle,
) -> (Vec<SdkSlashCommand>, Vec<SdkAgentInfo>, String, Vec<String>) {
    let command_registry = runtime.current_command_registry().await;
    let commands = command_registry
        .sdk_safe()
        .iter()
        .map(|cmd| SdkSlashCommand {
            name: cmd.base.name.clone(),
            description: cmd.base.description.clone(),
            argument_hint: cmd.base.argument_hint.clone().unwrap_or_default(),
        })
        .collect();

    let agent_catalog = runtime.agent_catalog_snapshot().await;
    let mut agents: Vec<SdkAgentInfo> = agent_catalog
        .active()
        .cloned()
        .map(def_to_sdk_agent_info)
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let engine_config = runtime.current_engine_config().await;
    let cwd = engine_config.workspace_cwd();
    let plugin_style_sources = runtime.project_services().output_style_sources();
    let output_style_manager =
        build_output_style_manager(runtime.runtime_config(), &cwd, &plugin_style_sources);
    let output_style = output_style_manager.active_name_for_sdk();
    let mut available_output_styles = output_style_manager.names();
    if !available_output_styles
        .iter()
        .any(|name| name == coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME)
    {
        available_output_styles
            .insert(0, coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME.to_string());
    }

    (commands, agents, output_style, available_output_styles)
}

async fn runtime_fast_mode_state(runtime: &SessionHandle) -> coco_types::FastModeState {
    if runtime.current_engine_config().await.fast_mode {
        coco_types::FastModeState::On
    } else {
        coco_types::FastModeState::Off
    }
}

/// `session/start` — create a new SDK session.
///
/// Legacy fallback installs the session in the scoped SDK state maps and
/// returns a generated `session_id`. Runtime-backed AppServer start uses the
/// replacement path and installs the same keyed SDK state after the AppServer
/// live slot commits.
pub(super) async fn handle_session_start(
    _params: coco_types::SessionStartParams,
    _ctx: &HandlerContext,
) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: "session/start requires AppServer lifecycle routing".into(),
        data: Some(serde_json::json!({ "kind": "app_server_required" })),
    }
}

pub(crate) async fn prepare_session_start(
    params: coco_types::SessionStartParams,
    state: &SdkServerState,
    connection_profile: &coco_types::ConnectionProfile,
) -> Result<PreparedStartSession, HandlerResult> {
    let session_id = coco_types::SessionId::generate();
    let cwd = match params.cwd.clone() {
        Some(cwd) => cwd,
        None => match state.workspace_cwd().await {
            Ok(cwd) => cwd.to_string_lossy().into_owned(),
            Err(err) => return Err(err),
        },
    };
    let model = params
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_SDK_MODEL.into());

    info!(session_id = %session_id, cwd = %cwd, model = %model, "SdkServer: session/start");

    let initialize = connection_profile.initialize();
    Ok(PreparedStartSession {
        session_id: session_id.clone(),
        cwd,
        model,
        permission_mode: params.permission_mode,
        agent_progress_summaries_enabled: initialize.agent_progress_summaries.unwrap_or(false),
    })
}

pub(crate) async fn install_scoped_started_session_state(
    state: &SdkServerState,
    prepared: &PreparedStartSession,
    runtime: &crate::session_runtime::SessionHandle,
) {
    configure_started_session_runtime(prepared, runtime).await;
    runtime.reset_session_accounting();
    state.touch_session_activity(prepared.session_id.clone());
}

async fn configure_started_session_runtime(
    prepared: &PreparedStartSession,
    runtime: &crate::session_runtime::SessionHandle,
) {
    let model_id = prepared.model.clone();
    let permission_mode = prepared.permission_mode;
    runtime
        .update_engine_config(move |config| {
            config.model_id = model_id;
            if let Some(permission_mode) = permission_mode {
                config.permission_mode = permission_mode;
            }
        })
        .await;
    let app_state = runtime.app_state();
    if let Some(mode) = prepared.permission_mode {
        // Brand-new session: the engine config / rules aren't wired yet, so the
        // Auto-entry stash starts empty. The evaluator-facing strip in
        // ToolContextFactory::build (keyed on live mode==Auto) is the real guard.
        crate::live_permission_mode::apply_to_app_state(
            app_state,
            coco_types::PermissionMode::Default,
            mode,
            &coco_types::PermissionRulesBySource::new(),
            coco_permissions::PlanModeAutoOptions::default(),
        )
        .await;
    }
    // Copy the SDK-level agentProgressSummaries flag onto the new
    // session's ToolAppState so the bg AgentTool path can gate
    // periodic-summary timers without reaching into SdkServerState.
    // Coordinator mode auto-enables independently.
    if prepared.agent_progress_summaries_enabled {
        app_state.write().await.agent_progress_summaries_enabled = true;
    }
}

/// Drain per-turn CoreEvents and forward to the outbound notification
/// channel, intercepting session envelope events.
///
/// Specifically:
/// - `SessionResult` events are **not** forwarded. Instead, their stats
///   are folded into the selected runtime's accounting. The aggregated
///   `SessionResult` is emitted once
///   when `session/archive` runs.
/// - `SessionStarted` events are also swallowed (defensive — the current
///   runner doesn't emit them, but if a future runner enables the
///   bootstrap path, we still want exactly one per session from the SDK
///   server side, not one per turn).
/// - All other events pass through unchanged.
///
/// `owner_session_id` is the session this forwarder was created for and is
/// used to stamp every outbound envelope.
pub(super) async fn forward_turn_events(
    mut rx: mpsc::Receiver<CoreEvent>,
    tx: mpsc::Sender<OutboundMessage>,
    session: crate::session_runtime::SessionHandle,
    owner_session_id: coco_types::SessionId,
) {
    use coco_types::ServerNotification;
    // Clear the active-turn slot on the FIRST terminal `TurnEnded` only, so a
    // stray second terminal event can never wipe a fast next turn's slot.
    let mut turn_slot_cleared = false;
    while let Some(event) = rx.recv().await {
        match event {
            CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
                session.accumulate_session_result(&params);
                // Swallow — aggregated result is emitted by session/archive.
            }
            CoreEvent::Protocol(ServerNotification::SessionStarted(_)) => {
                // Swallow: SessionStarted is owned by the SDK server, not the engine.
            }
            other => {
                // free the per-session turn slot BEFORE forwarding the
                // terminal `TurnEnded`, so the client's next `turn/start` (sent
                // the instant it sees `TurnEnded`) finds the slot free instead of
                // racing the turn task's own clear and hitting
                // `TurnAlreadyRunning`. This is now the sole clear site for the
                // active-turn record; the turn/compact tasks no longer clear it.
                if !turn_slot_cleared
                    && matches!(
                        &other,
                        CoreEvent::Protocol(ServerNotification::TurnEnded(_))
                    )
                {
                    session.clear_active_turn();
                    turn_slot_cleared = true;
                }
                if send_session_event(&tx, owner_session_id.clone(), other)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    if !turn_slot_cleared {
        // Returning from TurnRunner closes this channel and is also a clean
        // completion signal. A custom runner that omits TurnEnded must not
        // permanently occupy the session's active-turn slot.
        session.clear_active_turn();
    }
}

/// `session/archive` — clear keyed session state.
///
/// Emits the aggregated `SessionResult` (built from the session's
/// accumulated stats) as a final notification before clearing session state.
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
    let session_id = params.target.session_id().clone();
    // `archive_active_session` holds the active-session write lock for the
    // entire archive operation:
    // (1) validate session_id,(2) build the aggregate from a
    // consistent snapshot,(3) clear the slot. This closes the
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
    let Some(target_session_id) = ctx.target_session_id.as_ref() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_PARAMS,
            message: "session/archive requires an explicit target".into(),
            data: Some(serde_json::json!({ "kind": "missing_session_target" })),
        };
    };
    if target_session_id != &session_id {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "session_id mismatch: target is {target_session_id}, archive requested for {session_id}"
            ),
            data: None,
        };
    }
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("session {session_id} has no live runtime"),
            data: Some(serde_json::json!({ "kind": "session_runtime_not_found" })),
        };
    };
    let result_params = build_aggregated_session_result(&runtime, "archived");
    let active_turn = runtime.take_active_turn();
    ctx.state.forget_session_activity(&session_id);
    info!(session_id = %session_id, "SdkServer: session/archive");

    // Cancel any running turn. Outside the lock because:
    //  (a) `CancellationToken::cancel` is cheap and non-blocking
    //  (b) the turn task's subsequent cleanup also takes the session
    //       write lock for the cross-session guard — holding it here
    //       would deadlock
    if let Some(active_turn) = &active_turn {
        active_turn.cancel_token.cancel();
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
    if let Some(active_turn) = active_turn {
        match tokio::time::timeout(std::time::Duration::from_secs(5), active_turn.turn_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "session/archive: turn task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                "session/archive: turn task did not exit within 5s of cancel; \
                 emitting aggregate anyway (late events may still follow)"
            ),
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            active_turn.forwarder_task,
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "session/archive: forwarder task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                "session/archive: forwarder task did not drain within 5s"
            ),
        }
    }

    // Delete the persisted session record if a SessionManager is wired.
    // Non-fatal — log and continue if disk delete fails. Runs on the
    // blocking pool to avoid stalling the tokio worker on `remove_file`.
    // Snapshot the manager before the blocking call so disk I/O does
    // not serialize other session-manager readers.
    let manager_arc = ctx.state.session_manager_snapshot().await;
    if let Some(manager) = manager_arc {
        let target_id = session_id.as_str().to_string();
        let delete_result = tokio::task::spawn_blocking(move || manager.delete(&target_id)).await;
        match delete_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!(
                session_id = %session_id,
                error = %e,
                "session/archive: failed to delete persisted session record"
            ),
            Err(join_err) => warn!(
                session_id = %session_id,
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
    let _ = send_session_event_and_wait(&ctx.notif_tx, session_id, result_event).await;

    HandlerResult::ok_empty()
}

/// `session/rename` — persist a user-visible title for the active session.
pub(super) async fn handle_session_rename(
    params: coco_types::SessionRenameParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let Some(runtime) = runtime else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session runtime".into(),
            data: None,
        };
    };
    match crate::session_rename::persist_resolved_rename(&runtime, params.name).await {
        Ok(name) => {
            info!(name = %name, "SdkServer: session/rename");
            HandlerResult::ok(coco_types::SessionRenameResult { name })
        }
        Err(
            error @ (crate::session_rename::RenamePersistenceError::EmptyName
            | crate::session_rename::RenamePersistenceError::TranscriptNotFound),
        ) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: error.user_message(),
            data: None,
        },
        Err(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: error.user_message(),
            data: None,
        },
    }
}

/// `session/toggleTag` — toggle a tag on the active persisted session.
pub(super) async fn handle_session_toggle_tag(
    params: coco_types::SessionToggleTagParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let tag = params.tag.trim().to_string();
    if tag.is_empty() {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/toggleTag requires a non-empty tag".into(),
            data: None,
        };
    }
    let runtime = ctx.resolve_runtime().await;
    let Some(runtime) = runtime else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session runtime".into(),
            data: None,
        };
    };
    let session_id = runtime.current_typed_session_id().await;
    let manager = Arc::clone(runtime.session_manager());
    let tag_for_toggle = tag.clone();
    let session_id_for_toggle = session_id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        manager.toggle_tag(&session_id_for_toggle, &tag_for_toggle)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|inner| inner.map_err(anyhow::Error::from));
    match result {
        Ok((_, added)) => {
            info!(session_id = %session_id, tag = %tag, added, "SdkServer: session/toggleTag");
            HandlerResult::ok(coco_types::SessionToggleTagResult { tag, added })
        }
        Err(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("session/toggleTag failed for {session_id}: {error}"),
            data: None,
        },
    }
}

/// Build a final `SessionResultParams` from state-level SDK accounting.
/// Used by `session/archive` to synthesize the once-per-session aggregate
/// the SDK client expects.
pub(crate) fn build_aggregated_session_result(
    session: &crate::session_runtime::SessionHandle,
    default_stop_reason: &str,
) -> coco_types::SessionResultParams {
    let crate::session_runtime::SessionAccounting { started_at, stats } =
        session.session_accounting_snapshot();
    coco_types::SessionResultParams {
        session_id: session.session_id().clone(),
        total_turns: stats.total_turns,
        duration_ms: started_at.elapsed().as_millis() as i64,
        duration_api_ms: stats.total_duration_api_ms,
        is_error: stats.had_error,
        stop_reason: stats
            .last_stop_reason
            .clone()
            .unwrap_or_else(|| default_stop_reason.into()),
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
pub(crate) fn session_to_summary(
    s: &coco_session::Session,
) -> Result<coco_types::SdkSessionSummary, String> {
    session_data::session_record_to_summary(s)
}

/// `session/list` — enumerate persisted sessions, newest first.
///
/// Delegates to `SessionManager::list()`. Returns an empty list if no
/// manager is wired (session persistence disabled).
///
/// Errors:
/// - `INTERNAL_ERROR` if `SessionManager::list()` fails (e.g. filesystem error)
pub(super) async fn handle_session_list(ctx: &HandlerContext) -> HandlerResult {
    let manager = ctx.state.session_manager_snapshot().await;
    if manager.is_none() {
        info!("SdkServer: session/list (no session manager installed, returning empty)");
    }
    match session_data::persisted_session_list(manager).await {
        Ok(result) => {
            info!(count = result.sessions.len(), "SdkServer: session/list");
            HandlerResult::ok(result)
        }
        Err(error) => persisted_session_data_error(error),
    }
}

/// `session/read` — load a single persisted session's metadata plus transcript
/// messages.
///
/// Errors:
/// - `INVALID_REQUEST` if no session manager is wired
/// - `INVALID_REQUEST` if the session_id is not found on disk
/// - `INVALID_REQUEST` if the cursor or limit is invalid
pub(super) async fn handle_session_read(
    params: coco_types::SessionReadParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_data::persisted_session_read(ctx.state.session_manager_snapshot().await, &params)
        .await
    {
        Ok(result) => {
            info!(session_id = %params.target.session_id, "SdkServer: session/read");
            HandlerResult::ok(result)
        }
        Err(error) => persisted_session_data_error(error),
    }
}

/// `session/turns/list` — list derived transcript turn spans.
///
/// Errors:
/// - `INVALID_REQUEST` if no session manager is wired
/// - `INVALID_REQUEST` if the session_id is not found on disk
/// - `INVALID_REQUEST` if the cursor or limit is invalid
pub(super) async fn handle_session_turns_list(
    params: coco_types::SessionTurnsListParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_data::persisted_session_turns_list(
        ctx.state.session_manager_snapshot().await,
        &params,
    )
    .await
    {
        Ok(result) => {
            info!(session_id = %params.target.session_id, "SdkServer: session/turns/list");
            HandlerResult::ok(result)
        }
        Err(error) => persisted_session_data_error(error),
    }
}

fn persisted_session_data_error(error: PersistedSessionDataError) -> HandlerResult {
    HandlerResult::Err {
        code: error.code,
        message: error.message,
        data: None,
    }
}

/// `session/resume` — load a persisted session from disk and install
/// it as the active session, including the JSONL message history so
/// the next turn the SDK client drives sees the prior chain.
///
/// Replaces the current active session id (if any) and installs the
/// a fresh runtime for the resumed id. Any in-flight turn on the previous
/// session is cancelled first to prevent orphaned state.
/// When a `SessionRuntime` is already on the requested id, `runtime.history()`
/// is seeded with the loaded messages; mismatched runtime-backed resume must
/// use the AppServer runtime-replacement path.
/// The transcript dedup set is pre-populated so the per-turn JSONL append
/// doesn't re-write entries already on disk.
///
/// Errors:
/// - `INVALID_REQUEST` if no session manager is wired
/// - `INVALID_REQUEST` if the session_id is not found on disk
/// - `INTERNAL_ERROR` if the session manager's resume operation fails
///   or the JSONL transcript fails to load
pub(super) async fn handle_session_resume(
    _params: coco_types::SessionResumeParams,
    _ctx: &HandlerContext,
) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: "session/resume requires AppServer lifecycle routing".into(),
        data: Some(serde_json::json!({ "kind": "app_server_required" })),
    }
}

pub(crate) async fn load_resume_session(
    params: coco_types::SessionResumeParams,
    state: &Arc<SdkServerState>,
) -> Result<LoadedResumeSession, HandlerResult> {
    let Some(manager) = state.session_manager_snapshot().await else {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session persistence is not enabled on this server".into(),
            data: None,
        });
    };
    let memory_base = manager.memory_base().to_path_buf();
    let manager_arc = Arc::clone(&manager);
    let target_id = params.target.session_id.as_str().to_string();
    let resume_result = tokio::task::spawn_blocking(move || manager_arc.resume(&target_id)).await;
    let session = match resume_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err(HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!("session/resume: {e}"),
                data: None,
            });
        }
        Err(join_err) => {
            return Err(HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("session/resume task panicked: {join_err}"),
                data: None,
            });
        }
    };
    let session_id = match coco_types::SessionId::try_new(session.id.clone()) {
        Ok(id) => id,
        Err(e) => {
            return Err(HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!("session/resume: invalid persisted session id: {e}"),
                data: None,
            });
        }
    };

    // Skip-ahead the durable `session_seq` counter above the prior epoch's
    // watermark before the resumed runtime emits any durable event, so a
    // restarted process never re-issues a seq at or below one already shipped
    // to the hub.
    if let Some(watermark) = session.session_seq_watermark {
        state
            .session_seq_allocator()
            .initialize_after_watermark(&session_id, watermark);
    }

    // Load the JSONL transcript so the resumed run sees its own
    // history. Resume must fail if the transcript cannot be loaded;
    // silently starting with empty history makes "resume" behave like
    // a fresh session under an old id.
    //
    // The transcript lives in the resumed session's own project tree
    // (`<memory_base>/projects/<slug>/<sid>.jsonl`). Route through
    // `resolve_session_file_path` so a linked worktree (whose long cwd
    // path produces a different djb2 slug suffix than its sibling repo)
    // still resolves to the right file.
    let transcript_path = coco_session::storage::resolve_session_file_path(
        &memory_base,
        &session.id,
        Some(&session.working_dir),
    )
    .ok()
    .flatten()
    .map(|r| r.file_path);
    let Some(transcript_path) = transcript_path.as_ref() else {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "session/resume: transcript for {} was not found",
                session.id
            ),
            data: None,
        });
    };
    let conversation = match coco_session::recovery::load_conversation_for_resume(transcript_path) {
        Ok(r) => r,
        Err(e) => {
            return Err(HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!("session/resume: transcript load failed: {e}"),
                data: None,
            });
        }
    };

    Ok(LoadedResumeSession {
        session,
        session_id,
        conversation,
    })
}

pub(crate) async fn hydrate_runtime_for_resume_messages(
    session: &crate::session_runtime::SessionHandle,
    session_id: &coco_types::SessionId,
    prior_messages: &[coco_messages::Message],
) {
    let runtime = session;
    {
        let mut history = runtime.history().lock().await;
        history.clear();
        for message in prior_messages.iter().cloned() {
            history.push(message);
        }
    }
    runtime
        .seed_transcript_dedup(prior_messages.iter().filter_map(|m| m.uuid().copied()))
        .await;
    runtime
        .seed_tool_result_replacement_state(prior_messages, session_id, None)
        .await;

    if prior_messages.is_empty() {
        return;
    }
    let cfg = runtime.current_engine_config().await;
    let messages = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();
    let goal = crate::goal_command::restore_goal_from_history(
        &messages,
        runtime.app_state(),
        &runtime.hook_registry(),
        runtime.session_usage_snapshot().await.totals.output_tokens,
        crate::goal_command::GoalGate {
            hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
            trust_rejected: false,
        },
    )
    .await;
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}
