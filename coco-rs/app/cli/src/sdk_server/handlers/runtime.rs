//! Runtime-state mutations (`setModel` / `setPermissionMode` / `setThinking`
//! / `updateEnv` / `stopTask`) plus observability and lightweight stub
//! handlers (`context/usage`, `plugin/reload`, `config/applyFlags`).

use tracing::info;

use super::DEFAULT_SDK_MODEL;
use super::HandlerContext;
use super::HandlerResult;

/// `control/setModel` — mutate the active session's model.
///
/// The updated model takes effect on the *next* `turn/start`. In-flight
/// turns continue running against the previous model (they'd need
/// restarting to swap models mid-call).
///
/// Passing `None` means "revert to the default model", which we
/// interpret as `claude-opus-4-6` (the bootstrap default from
/// `handle_session_start`).
pub(super) async fn handle_set_model(
    params: coco_types::SetModelParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let mut slot = ctx.state.session.write().await;
    let Some(session) = slot.as_mut() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    let new_model = params
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_SDK_MODEL.into());
    info!(
        session_id = %session.session_id,
        old_model = %session.model,
        new_model = %new_model,
        "SdkServer: control/setModel"
    );
    session.model = new_model;
    HandlerResult::ok_empty()
}

/// `control/setPermissionMode` — mutate the session's permission mode.
///
/// Writes:
/// 1. [`SessionHandle::permission_mode`] — session-scoped override read
///    by `sdk_runner::run_turn` as a fallback when the turn params
///    don't carry an explicit mode.
/// 2. [`SessionHandle::app_state`] `permission_mode` — the engine's
///    live mode source of truth. Updating it mid-session propagates
///    to any in-flight engine's next `create_tool_context` read.
///    Without this write, mid-session toggles are invisible to the
///    plan-mode reminder + permission evaluator.
/// 3. Applies the same plan/auto transition side effects as the TUI
///    path: entering Plan stashes `pre_plan_mode` and stamps
///    `plan_mode_entry_ms`; leaving Plan schedules the one-shot exit
///    banner; leaving Auto clears `stripped_dangerous_rules`.
pub(super) async fn handle_set_permission_mode(
    params: coco_types::SetPermissionModeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Mid-session bypass guard: reject any attempt to escalate into
    // `BypassPermissions` when the session was not launched with one
    // of the authorization flags. Catches accidental SDK clients and
    // closes the ungated-bypass surface exposed by the TUI plan-exit
    // prompt before its fix.
    if params.mode == coco_types::PermissionMode::BypassPermissions
        && !ctx
            .state
            .bypass_permissions_available
            .load(std::sync::atomic::Ordering::Relaxed)
    {
        return HandlerResult::Err {
            code: coco_types::error_codes::PERMISSION_DENIED,
            message: "Cannot set permission mode to bypassPermissions because \
                      the session was not launched with \
                      --dangerously-skip-permissions (or \
                      --allow-dangerously-skip-permissions)."
                .into(),
            data: None,
        };
    }

    let mut slot = ctx.state.session.write().await;
    let Some(session) = slot.as_mut() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    info!(
        session_id = %session.session_id,
        mode = ?params.mode,
        "SdkServer: control/setPermissionMode"
    );
    session.permission_mode = Some(params.mode);

    let app_state = session.app_state.clone();
    // Drop the session write lock before taking app_state's lock to
    // keep lock order consistent (session → app_state never inverted).
    drop(slot);
    // Provenance for the Auto-entry dangerous-rule strip must come from the
    // SAME live base the transition writes — this session's `app_state` (the
    // per-SessionHandle base the engine runs against), NOT the SessionRuntime
    // base. Reading the session's own base keeps strip/restore coherent.
    let live_allow_rules = app_state.read().await.permissions.allow_rules.clone();
    let change = crate::live_permission_mode::apply_to_app_state(
        &app_state,
        coco_types::PermissionMode::Default,
        params.mode,
        &live_allow_rules,
    )
    .await;
    crate::live_permission_mode::publish_outbound_if_changed(
        &ctx.notif_tx,
        params.mode,
        crate::live_permission_mode::sdk_bypass_available(&ctx.state),
        change.changed,
    )
    .await;

    HandlerResult::ok_empty()
}

/// `control/setThinking` — mutate the session's thinking level.
///
/// `thinking_level = None` clears the override so turns fall back to
/// the engine's default.
pub(super) async fn handle_set_thinking(
    params: coco_types::SetThinkingParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let mut slot = ctx.state.session.write().await;
    let Some(session) = slot.as_mut() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    info!(
        session_id = %session.session_id,
        level = ?params.thinking_level,
        "SdkServer: control/setThinking"
    );
    session.thinking_level = params.thinking_level;
    HandlerResult::ok_empty()
}

/// `control/stopTask` — cooperative cancellation of a specific task.
///
/// Coco-rs's in-process background task registry isn't threaded through
/// the SDK server yet, so this is structurally equivalent to
/// `turn/interrupt`: we cancel any in-flight turn so the runner unwinds.
/// The `task_id` is logged for later correlation once the task manager
/// is wired through `SdkServerState`.
pub(super) async fn handle_stop_task(
    params: coco_types::StopTaskParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let slot = ctx.state.session.read().await;
    let Some(session) = slot.as_ref() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    match &session.active_turn_cancel {
        Some(token) => {
            info!(
                session_id = %session.session_id,
                task_id = %params.task_id,
                "SdkServer: control/stopTask (cancels active turn)"
            );
            token.cancel();
            HandlerResult::ok_empty()
        }
        None => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("no task in flight matching task_id {}", params.task_id),
            data: None,
        },
    }
}

/// `control/updateEnv` — merge environment variable updates into the
/// session's override map.
///
/// Passing an empty string for a value is interpreted as "unset" and
/// removes the key from the override map. The resulting map is passed
/// to tool invocations when the shell executor is wired to read it.
pub(super) async fn handle_update_env(
    params: coco_types::UpdateEnvParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let mut slot = ctx.state.session.write().await;
    let Some(session) = slot.as_mut() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    let mut applied = 0;
    let mut cleared = 0;
    for (k, v) in params.env {
        if v.is_empty() {
            if session.env_overrides.remove(&k).is_some() {
                cleared += 1;
            }
        } else {
            session.env_overrides.insert(k, v);
            applied += 1;
        }
    }
    info!(
        session_id = %session.session_id,
        applied,
        cleared,
        total = session.env_overrides.len(),
        "SdkServer: control/updateEnv"
    );
    HandlerResult::ok_empty()
}

/// `agent/interruptCurrentWork` — abort one teammate's current turn
/// without killing the teammate lifecycle.
///
/// Escape while viewing a teammate aborts the current work controller,
/// whereas Ctrl+C still kills agents via the broader cancellation path.
pub(super) async fn handle_agent_interrupt_current_work(
    params: coco_types::AgentInterruptCurrentWorkParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let Some(runtime) = ctx.state.session_runtime.read().await.clone() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "agent teams are not active for this session".into(),
            data: None,
        };
    };

    match runtime.interrupt_agent_current_work(&params.agent_id).await {
        Ok(true) => HandlerResult::ok_empty(),
        Ok(false) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "agent {} has no active current work to interrupt",
                params.agent_id
            ),
            data: None,
        },
        Err(message) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message,
            data: None,
        },
    }
}

/// `context/usage` — return the active session's current Main context view.
pub(super) async fn handle_context_usage(ctx: &HandlerContext) -> HandlerResult {
    let (history_handle, app_state) = {
        let slot = ctx.state.session.read().await;
        let Some(session) = slot.as_ref() else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session; call session/start first".into(),
                data: None,
            };
        };
        (session.history.clone(), session.app_state.clone())
    };
    let history_arcs = history_handle.lock().await.clone();
    let Some(runtime) = ctx.state.session_runtime.read().await.clone() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "context usage requires an active session runtime".into(),
            data: None,
        };
    };
    let history = coco_messages::MessageHistory::from_arcs_preserving_latest_usage(history_arcs);
    match runtime
        .analyze_context_snapshot(history, Some(app_state))
        .await
    {
        Ok(report) => HandlerResult::ok(report.to_wire()),
        Err(err) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: err.to_string(),
            data: None,
        },
    }
}

/// `plugin/reload` — hot-reload plugins.
///
/// Mirrors the TUI `/reload-plugins` chain (`tui_runner::run_reload_plugins`)
/// against the process-shared `SessionRuntime`: reload plugins (commands +
/// skills) → agent catalog → LSP servers → hooks, then report the live
/// command/agent/plugin snapshots. When no `SessionRuntime` is wired (e.g.
/// handler-level test harnesses), acks with an empty result.
pub(super) async fn handle_plugin_reload(ctx: &HandlerContext) -> HandlerResult {
    let runtime_arc = {
        let slot = ctx.state.session_runtime.read().await;
        slot.as_ref().cloned()
    };
    let Some(runtime) = runtime_arc else {
        info!("SdkServer: plugin/reload (no SessionRuntime wired, returning empty)");
        return HandlerResult::ok(coco_types::PluginReloadResult {
            plugins: Vec::new(),
            commands: Vec::new(),
            agents: Vec::new(),
            error_count: 0,
        });
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let command_count = runtime.reload_plugins(&cwd).await;
    runtime.reload_agent_catalog().await;
    runtime.reload_lsp_servers(&cwd).await;
    let error_count = match runtime.reload_hooks().await {
        Ok(_) => 0,
        Err(e) => {
            tracing::warn!(target: "coco::plugins", error = %e, "SDK plugin/reload: hook reload failed");
            1
        }
    };

    // Enumerate the live registry/catalog snapshots for the result.
    let command_registry = runtime.current_command_registry().await;
    let commands: Vec<String> = command_registry
        .snapshot_for_ui()
        .into_iter()
        .map(|c| c.name)
        .collect();
    let agent_catalog = runtime.current_agent_catalog().await;
    let agents: Vec<String> = agent_catalog.active().map(|a| a.name.clone()).collect();
    let config_home = runtime.config_home.clone();
    let project_dir = runtime
        .current_engine_config()
        .await
        .project_dir
        .unwrap_or_else(|| cwd.clone());
    let plugins: Vec<String> = coco_plugins::load_all_installed_plugins(&config_home, &project_dir)
        .iter()
        .map(|p| p.id.to_string())
        .collect();

    info!(
        commands = command_count,
        agents = agents.len(),
        plugins = plugins.len(),
        error_count,
        "SdkServer: plugin/reload"
    );
    HandlerResult::ok(coco_types::PluginReloadResult {
        plugins,
        commands,
        agents,
        error_count,
    })
}

/// `config/applyFlags` — apply runtime feature-flag settings.
///
/// Currently logs the flags and acks. A follow-up could merge them into
/// a runtime overrides map on `SdkServerState` so other handlers see
/// the effective values.
pub(super) async fn handle_config_apply_flags(
    params: coco_types::ConfigApplyFlagsParams,
    _ctx: &HandlerContext,
) -> HandlerResult {
    info!(
        count = params.settings.len(),
        "SdkServer: config/applyFlags (logged; no runtime override map yet)"
    );
    HandlerResult::ok_empty()
}
