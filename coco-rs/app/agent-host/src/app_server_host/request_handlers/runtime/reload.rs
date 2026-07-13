use tracing::info;

use super::session_control_error;
use crate::app_server_host::outbound::send_session_event;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_controls;

const FAST_MODE_FLAG_SNAKE: &str = "fast_mode";
const FAST_MODE_FLAG_CAMEL: &str = "fastMode";

/// `plugin/reload` — hot-reload plugins.
pub(crate) async fn handle_plugin_reload(ctx: &HandlerContext) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let had_runtime = runtime.is_some();
    let result = session_controls::reload_plugins(runtime).await;
    if !had_runtime {
        info!("AppServerHost: plugin/reload (no SessionRuntime wired, returning empty)");
    } else {
        info!(
            commands = result.commands.len(),
            agents = result.agents.len(),
            plugins = result.plugins.len(),
            error_count = result.error_count,
            "AppServerHost: plugin/reload"
        );
    }
    HandlerResult::ok(result)
}

/// `hook/reload` — rebuild the live `HookRegistry` from current settings.
pub(crate) async fn handle_hook_reload(ctx: &HandlerContext) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let had_runtime = runtime.is_some();
    match session_controls::reload_hooks(runtime).await {
        Ok(result) => {
            if !had_runtime {
                info!("AppServerHost: hook/reload (no SessionRuntime wired, returning empty)");
            } else {
                info!(hook_count = result.hook_count, "AppServerHost: hook/reload");
            }
            HandlerResult::ok(result)
        }
        Err(error) => session_control_error(error),
    }
}

/// `config/applyFlags` — apply runtime feature-flag settings.
///
/// Unknown flags are acknowledged for client compatibility. When a local
/// `SessionRuntime` is installed, the recognized `fast_mode` / `fastMode`
/// boolean updates the live engine config and publishes the same
/// `FastModeChanged` notification as the TUI direct path used to emit.
pub(crate) async fn handle_config_apply_flags(
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
        && let Some(session_id) =
            session_controls::set_fast_mode(ctx.resolve_runtime().await, active).await
    {
        let _ = send_session_event(
            &ctx.notif_tx,
            session_id,
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::FastModeChanged {
                active,
            }),
        )
        .await;
    }

    info!(
        count = params.settings.len(),
        fast_mode = ?fast_mode,
        "AppServerHost: config/applyFlags"
    );
    HandlerResult::ok_empty()
}
