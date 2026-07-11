//! Runtime-state mutations (`setModel` / `setModelRole` / `setPermissionMode`
//! / `setThinking` / `applyPermissionUpdate` / `updateEnv` / `stopTask`) plus
//! observability and runtime-backed handlers (`context/usage`,
//! `plugin/reload`, `hook/reload`, `config/applyFlags`).

use std::sync::Arc;

use tracing::info;

use super::{DEFAULT_SDK_MODEL, HandlerContext, HandlerResult};
use crate::sdk_server::outbound::send_session_event;
use coco_tool_runtime::TaskHandle;

const FAST_MODE_FLAG_SNAKE: &str = "fast_mode";
const FAST_MODE_FLAG_CAMEL: &str = "fastMode";

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
    let new_model = params
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_SDK_MODEL.into());

    if let Some(runtime_handle) = ctx.resolve_runtime().await {
        let session_id = runtime_handle.session_id().clone();
        let old_model = runtime_handle.current_engine_config().await.model_id;
        let model_for_config = new_model.clone();
        runtime_handle
            .update_engine_config(move |cfg| {
                cfg.model_id = model_for_config;
            })
            .await;
        // the metadata write must target the SAME routed session as the
        // engine-config write above (previously it wrote the scoped/active
        // session, a different session during replacement windows) so the
        // session/list overlay stays coherent with the runtime.
        ctx.state
            .update_session_model(&session_id, new_model.clone());
        info!(
            session_id = %session_id,
            old_model = %old_model,
            new_model = %new_model,
            "SdkServer: control/setModel"
        );
        return HandlerResult::ok_empty();
    }

    let Some(session_id) = ctx.active_session_id().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    let old_model = ctx
        .state
        .update_session_model(&session_id, new_model.clone())
        .unwrap_or_else(|| DEFAULT_SDK_MODEL.into());
    info!(
        session_id = %session_id,
        old_model = %old_model,
        new_model = %new_model,
        "SdkServer: control/setModel"
    );
    HandlerResult::ok_empty()
}

/// `control/setModelRole` — apply an in-memory role/provider/model override.
pub(super) async fn handle_set_model_role(
    params: coco_types::SetModelRoleParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(session_runtime) = runtime_arc else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no session runtime installed".into(),
            data: None,
        };
    };
    let runtime = &session_runtime;
    let moa_endpoint = if params.provider == "moa" {
        runtime
            .runtime_config()
            .model_roles
            .moa_preset(&params.model_id)
            .cloned()
    } else {
        None
    };
    let (acting_provider, acting_model_id, display_provider, display_model_id) =
        if let Some(endpoint) = moa_endpoint.as_ref() {
            (
                endpoint.aggregator.provider.clone(),
                endpoint.aggregator.model_id.clone(),
                params.provider.clone(),
                params.model_id.clone(),
            )
        } else {
            (
                params.provider.clone(),
                params.model_id.clone(),
                params.provider.clone(),
                params.model_id.clone(),
            )
        };
    let display_name = runtime
        .runtime_config()
        .model_registry
        .resolve(&acting_provider, &acting_model_id)
        .map(|resolved| {
            resolved
                .info
                .display_name
                .clone()
                .unwrap_or_else(|| acting_model_id.clone())
        })
        .unwrap_or_else(|| acting_model_id.clone());
    let context_window = runtime
        .runtime_config()
        .model_registry
        .resolve(&acting_provider, &acting_model_id)
        .map(|resolved| resolved.info.context_window.get() as i64);
    let api = runtime
        .runtime_config()
        .providers
        .get(&acting_provider)
        .map(|provider| provider.api)
        .unwrap_or(coco_types::ProviderApi::Anthropic);
    let spec = coco_types::ModelSpec {
        provider: acting_provider.clone(),
        api,
        model_id: acting_model_id.clone(),
        display_name: display_name.clone(),
    };
    if let Err(error) = runtime
        .apply_role_override(
            params.role,
            crate::session_runtime::RoleOverride {
                spec,
                effort: params.effort,
            },
        )
        .await
    {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "failed to apply {role} -> {provider}/{model_id}: {error}",
                role = params.role.as_str(),
                provider = display_provider,
                model_id = display_model_id,
            ),
            data: None,
        };
    }
    runtime
        .model_runtimes()
        .set_role_moa_endpoint_override(params.role, moa_endpoint);
    // keep the scoped metadata model in step with a Main-role change so the
    // session/list overlay (which reads metadata.model) does not diverge from
    // the runtime after a `setModelRole` on the main role.
    if params.role == coco_types::ModelRole::Main {
        ctx.state
            .update_session_model(session_runtime.session_id(), display_model_id.clone());
    }
    info!(
        role = %params.role.as_str(),
        provider = %display_provider,
        model_id = %display_model_id,
        effort = ?params.effort,
        "SdkServer: control/setModelRole"
    );

    let changed = coco_types::ModelRoleChangedParams {
        role: params.role,
        model_id: display_model_id,
        provider: display_provider,
        context_window,
        effort: params.effort,
    };
    let _ = send_session_event(
        &ctx.notif_tx,
        session_runtime.session_id().clone(),
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::ModelRoleChanged(
            changed.clone(),
        )),
    )
    .await;
    HandlerResult::ok(coco_types::SetModelRoleResult {
        changed,
        display_name,
    })
}

/// `control/setPermissionMode` — mutate the session's permission mode.
///
/// Writes:
/// 1. Runtime `QueryEngineConfig.permission_mode` when a runtime is installed.
/// 2. The live `ToolAppState.permission_mode` read by tool context creation.
/// 3. The legacy SDK session's shared app state when no runtime is installed.
/// 4. Applies the same plan/auto transition side effects as the TUI
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
        && !ctx.state.bypass_permissions_available()
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

    let runtime_arc = ctx.resolve_runtime().await;
    if let Some(runtime) = runtime_arc {
        let fallback_mode = runtime.current_engine_config().await.permission_mode;
        runtime
            .update_engine_config(move |cfg| cfg.permission_mode = params.mode)
            .await;

        let app_state = Arc::clone(runtime.app_state());
        let live_allow_rules = app_state.read().await.permissions.allow_rules.clone();
        let config = runtime.current_engine_config().await;
        let change = crate::live_permission_mode::apply_to_app_state(
            &app_state,
            fallback_mode,
            params.mode,
            &live_allow_rules,
            coco_permissions::PlanModeAutoOptions {
                use_auto_mode_during_plan: config.use_auto_mode_during_plan,
                auto_mode_available: config.permission_mode_availability.auto,
            },
        )
        .await;
        crate::live_permission_mode::publish_outbound_if_changed(
            &ctx.notif_tx,
            runtime.session_id().clone(),
            params.mode,
            crate::live_permission_mode::sdk_bypass_available(&ctx.state),
            change.changed,
        )
        .await;
        info!(
            session_id = %runtime.session_id(),
            mode = ?params.mode,
            "SdkServer: control/setPermissionMode"
        );
        return HandlerResult::ok_empty();
    }

    let Some(session_id) = ctx.active_session_id().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    info!(
        session_id = %session_id,
        mode = ?params.mode,
        "SdkServer: control/setPermissionMode"
    );
    let Some(handoff) = ctx.state.session_handoff_snapshot(&session_id) else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "session handoff state is missing".into(),
            data: None,
        };
    };
    let app_state = handoff.app_state;
    // Provenance for the Auto-entry dangerous-rule strip must come from the
    // SAME live base the transition writes — this session's handoff
    // `app_state`, NOT the SessionRuntime base. Reading the session's own base
    // keeps strip/restore coherent.
    let (previous_mode, live_allow_rules) = {
        let guard = app_state.read().await;
        (
            guard
                .permissions
                .mode
                .unwrap_or(coco_types::PermissionMode::Default),
            guard.permissions.allow_rules.clone(),
        )
    };
    let change = crate::live_permission_mode::apply_to_app_state(
        &app_state,
        previous_mode,
        params.mode,
        &live_allow_rules,
        coco_permissions::PlanModeAutoOptions::default(),
    )
    .await;
    crate::live_permission_mode::publish_outbound_if_changed(
        &ctx.notif_tx,
        session_id.clone(),
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
    if let Some(runtime) = ctx.resolve_runtime().await {
        let session_id = runtime.session_id().clone();
        let thinking_level = params.thinking_level.clone();
        runtime
            .update_engine_config(move |cfg| {
                cfg.thinking_level = thinking_level;
            })
            .await;
        info!(
            session_id = %session_id,
            level = ?params.thinking_level,
            "SdkServer: control/setThinking"
        );
        publish_main_role_changed_for_thinking(ctx, &runtime, params.thinking_level).await;
        return HandlerResult::ok_empty();
    }

    let Some(session_id) = ctx.active_session_id().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };

    info!(
        session_id = %session_id,
        level = ?params.thinking_level,
        "SdkServer: control/setThinking"
    );

    HandlerResult::ok_empty()
}

/// `control/setAgentColor` — mutate the session's UI badge color.
pub(super) async fn handle_set_agent_color(
    params: coco_types::SetAgentColorParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(session_runtime) = runtime_arc else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no session runtime installed".into(),
            data: None,
        };
    };

    session_runtime.app_state().write().await.agent_color = params.color;
    info!("SdkServer: control/setAgentColor");
    HandlerResult::ok_empty()
}

/// `control/applyPermissionUpdate` — apply one permission editor update.
pub(super) async fn handle_apply_permission_update(
    params: coco_types::ApplyPermissionUpdateParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(session_runtime) = runtime_arc else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no session runtime installed".into(),
            data: None,
        };
    };
    session_runtime
        .apply_permission_updates_everywhere(std::slice::from_ref(&params.update))
        .await;
    info!("SdkServer: control/applyPermissionUpdate");
    HandlerResult::ok_empty()
}

/// `control/resetSessionPermissionRules` — clear session-scoped allow/deny rules.
pub(super) async fn handle_reset_session_permission_rules(ctx: &HandlerContext) -> HandlerResult {
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(session_runtime) = runtime_arc else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no session runtime installed".into(),
            data: None,
        };
    };

    let runtime = &session_runtime;
    let mut guard = runtime.app_state().write().await;
    let cleared_allow_rules = guard
        .permissions
        .allow_rules
        .remove(&coco_types::PermissionRuleSource::Session)
        .map_or(0, |rules| rules.len());
    let cleared_deny_rules = guard
        .permissions
        .deny_rules
        .remove(&coco_types::PermissionRuleSource::Session)
        .map_or(0, |rules| rules.len());
    drop(guard);

    info!(
        cleared_allow_rules,
        cleared_deny_rules, "SdkServer: control/resetSessionPermissionRules"
    );
    HandlerResult::ok(coco_types::ResetSessionPermissionRulesResult {
        cleared_allow_rules,
        cleared_deny_rules,
    })
}

async fn publish_main_role_changed_for_thinking(
    ctx: &HandlerContext,
    runtime: &crate::session_runtime::SessionHandle,
    thinking_level: Option<coco_types::ThinkingLevel>,
) {
    let Some(resolved) = runtime.resolve_role(coco_types::ModelRole::Main).await else {
        return;
    };
    let context_window = runtime
        .runtime_config()
        .model_registry
        .resolve(&resolved.spec.provider, &resolved.spec.model_id)
        .map(|model| model.info.context_window.get() as i64);
    let _ = send_session_event(
        &ctx.notif_tx,
        runtime.session_id().clone(),
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::ModelRoleChanged(
            coco_types::ModelRoleChangedParams {
                role: coco_types::ModelRole::Main,
                model_id: resolved.spec.model_id,
                provider: resolved.spec.provider,
                context_window,
                effort: thinking_level.map(|level| level.effort),
            },
        )),
    )
    .await;
}

/// `control/stopTask` — cooperative cancellation of a specific task.
///
/// When the local AppServer bridge has installed a [`SessionRuntime`],
/// route through its task registry so the target task's cancel token fires.
/// SDK-only sessions that have no installed runtime keep the legacy active-turn
/// fallback.
pub(super) async fn handle_stop_task(
    params: coco_types::StopTaskParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let Some(session_id) = ctx.active_session_id().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };

    if let Some(runtime) = ctx.resolve_runtime().await {
        let Some(task_runtime) = runtime.current_task_runtime().await else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "task runtime is not available for this session".into(),
                data: None,
            };
        };
        return match task_runtime.kill_task(&params.task_id).await {
            Ok(()) => {
                info!(
                    session_id = %session_id,
                    task_id = %params.task_id,
                    "SdkServer: control/stopTask (task runtime)"
                );
                HandlerResult::ok_empty()
            }
            Err(error) => HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!("control/stopTask: {error}"),
                data: None,
            },
        };
    }

    let token = {
        let Some(session) = ctx.resolve_runtime().await else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session".into(),
                data: None,
            };
        };
        session.active_turn_cancel_token()
    };

    match token {
        Some(token) => {
            info!(
                session_id = %session_id,
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

/// `control/updateEnv` — accept environment variable updates.
///
/// Passing an empty string for a value is interpreted as "unset" and
/// counted as a clear. When a `SessionRuntime` is installed, updates are
/// applied to the runtime-owned shell env store consumed by future shell
/// tool spawns. The no-runtime SDK fallback still acknowledges updates for
/// protocol compatibility, but has no shell provider to update.
pub(super) async fn handle_update_env(
    params: coco_types::UpdateEnvParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let (session_id, applied, cleared) = if let Some(runtime) = ctx.resolve_runtime().await {
        let session_id = runtime.current_typed_session_id().await;
        let (applied, cleared) = runtime.apply_session_env_updates(params.env);
        (session_id, applied, cleared)
    } else {
        let Some(session_id) = ctx.active_session_id().await else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session".into(),
                data: None,
            };
        };
        let mut applied = 0_i32;
        let mut cleared = 0_i32;
        for (_key, value) in params.env {
            if value.is_empty() {
                cleared += 1;
            } else {
                applied += 1;
            }
        }
        (session_id, applied, cleared)
    };
    info!(
        session_id = %session_id,
        applied,
        cleared,
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
    let Some(runtime) = ctx.resolve_runtime().await else {
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

/// `task/list` — list running/background tasks for the active session.
pub(super) async fn handle_task_list(ctx: &HandlerContext) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "task/list requires an active session runtime".into(),
            data: None,
        };
    };
    let Some(task_runtime) = runtime.current_task_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "task runtime is not available for this session".into(),
            data: None,
        };
    };
    let tasks = task_runtime.list_tasks().await;
    info!(count = tasks.len(), "SdkServer: task/list");
    HandlerResult::ok(coco_types::TaskListResult { tasks })
}

/// `task/detail` — read terminal outputs for one running/background task.
pub(super) async fn handle_task_detail(
    params: coco_types::TaskDetailParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "task/detail requires an active session runtime".into(),
            data: None,
        };
    };
    let Some(task_runtime) = runtime.current_task_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "task runtime is not available for this session".into(),
            data: None,
        };
    };
    match task_runtime.read_terminal_outputs(&params.task_id).await {
        Ok(outputs) => {
            info!(task_id = %params.task_id, "SdkServer: task/detail");
            HandlerResult::ok(coco_types::TaskDetailResult {
                task_id: params.task_id,
                stdout: outputs.stdout,
                stderr: outputs.stderr,
                exit_code: outputs.exit_code,
                interrupted: outputs.interrupted,
            })
        }
        Err(error) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("task/detail: {error}"),
            data: None,
        },
    }
}

/// `control/backgroundAllTasks` — detach every foreground task into the
/// background. No-op when this session has no task runtime installed.
pub(super) async fn handle_background_all_tasks(ctx: &HandlerContext) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::ok(coco_types::BackgroundAllTasksResult {
            task_ids: Vec::new(),
        });
    };
    let Some(task_runtime) = runtime.current_task_runtime().await else {
        return HandlerResult::ok(coco_types::BackgroundAllTasksResult {
            task_ids: Vec::new(),
        });
    };
    let task_ids = task_runtime.manager().background_all_foreground().await;
    info!(
        count = task_ids.len(),
        "SdkServer: control/backgroundAllTasks"
    );
    HandlerResult::ok(coco_types::BackgroundAllTasksResult { task_ids })
}

/// `session/cost` — return the active session's live usage/cost report.
pub(super) async fn handle_session_cost(ctx: &HandlerContext) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/cost requires an active session runtime".into(),
            data: None,
        };
    };
    let usage = runtime.session_usage_snapshot().await;
    let text = coco_messages::format_session_cost(&usage);
    info!("SdkServer: session/cost");
    HandlerResult::ok(coco_types::SessionCostResult { text, usage })
}

/// `session/status` — return the active session's live status report.
pub(super) async fn handle_session_status(ctx: &HandlerContext) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/status requires an active session runtime".into(),
            data: None,
        };
    };
    let text = runtime.status_report().await;
    info!("SdkServer: session/status");
    HandlerResult::ok(coco_types::SessionStatusResult { text })
}

/// `context/usage` — return the active session's current Main context view.
pub(super) async fn handle_context_usage(ctx: &HandlerContext) -> HandlerResult {
    let Some(runtime) = ctx.resolve_runtime().await else {
        let Some(_session_id) = ctx.active_session_id().await else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session; call session/start first".into(),
                data: None,
            };
        };
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "context usage requires an active session runtime".into(),
            data: None,
        };
    };
    match runtime.analyze_main_context().await {
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
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(runtime) = runtime_arc else {
        info!("SdkServer: plugin/reload (no SessionRuntime wired, returning empty)");
        return HandlerResult::ok(coco_types::PluginReloadResult {
            plugins: Vec::new(),
            commands: Vec::new(),
            agents: Vec::new(),
            error_count: 0,
        });
    };

    let cwd = runtime.original_cwd().clone();
    let command_count = runtime.reload_plugins(&cwd).await;
    runtime.reload_agent_catalog().await;
    runtime.reload_lsp_servers().await;
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
    let config_home = runtime.config_home().clone();
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

/// `hook/reload` — rebuild the live `HookRegistry` from current settings.
pub(super) async fn handle_hook_reload(ctx: &HandlerContext) -> HandlerResult {
    let runtime_arc = ctx.resolve_runtime().await;
    let Some(runtime) = runtime_arc else {
        info!("SdkServer: hook/reload (no SessionRuntime wired, returning empty)");
        return HandlerResult::ok(coco_types::HookReloadResult { hook_count: 0 });
    };

    match runtime.reload_hooks().await {
        Ok(hook_count) => {
            info!(hook_count, "SdkServer: hook/reload");
            HandlerResult::ok(coco_types::HookReloadResult {
                hook_count: i64::try_from(hook_count).unwrap_or(i64::MAX),
            })
        }
        Err(error) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: error.to_string(),
            data: None,
        },
    }
}

/// `config/applyFlags` — apply runtime feature-flag settings.
///
/// Unknown flags are acknowledged for SDK compatibility. When a local
/// `SessionRuntime` is installed, the recognized `fast_mode` / `fastMode`
/// boolean updates the live engine config and publishes the same
/// `FastModeChanged` notification as the TUI direct path used to emit.
pub(super) async fn handle_config_apply_flags(
    params: coco_types::ConfigApplyFlagsParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let fast_mode = match params
        .settings
        .get(FAST_MODE_FLAG_SNAKE)
        .or_else(|| params.settings.get(FAST_MODE_FLAG_CAMEL))
    {
        Some(value) => match value.as_bool() {
            Some(value) => Some(value),
            None => {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INVALID_PARAMS,
                    message: format!("config/applyFlags: {FAST_MODE_FLAG_SNAKE} must be a boolean"),
                    data: None,
                };
            }
        },
        None => None,
    };

    if let Some(active) = fast_mode
        && let Some(runtime) = ctx.resolve_runtime().await
    {
        runtime
            .update_engine_config(|cfg| {
                cfg.fast_mode = active;
            })
            .await;
        let _ = send_session_event(
            &ctx.notif_tx,
            runtime.session_id().clone(),
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::FastModeChanged {
                active,
            }),
        )
        .await;
    }

    info!(
        count = params.settings.len(),
        fast_mode = ?fast_mode,
        "SdkServer: config/applyFlags"
    );
    HandlerResult::ok_empty()
}
