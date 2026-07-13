use tracing::info;

use super::session_control_error;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_controls;

/// `control/setPermissionMode` — mutate the session's permission mode.
///
/// Writes:
/// 1. Runtime `QueryEngineConfig.permission_mode`.
/// 2. The live `ToolAppState.permission_mode` read by tool context creation.
/// 3. Applies the same plan/auto transition side effects as the TUI
///    path: entering Plan stashes `pre_plan_mode` and stamps
///    `plan_mode_entry_ms`; leaving Plan schedules the one-shot exit
///    banner; leaving Auto clears `stripped_dangerous_rules`.
pub(crate) async fn handle_set_permission_mode(
    params: coco_types::SetPermissionModeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Mid-session bypass guard: reject any attempt to escalate into
    // `BypassPermissions` when the session was not launched with one
    // of the authorization flags. Catches accidental remote clients and
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

    match session_controls::set_permission_mode(ctx.resolve_runtime().await, params.mode).await {
        Ok(result) => {
            crate::live_permission_mode::publish_outbound_if_changed(
                &ctx.notif_tx,
                result.session_id.clone(),
                params.mode,
                ctx.state.bypass_permissions_available(),
                result.changed,
            )
            .await;
            info!(
                session_id = %result.session_id,
                mode = ?params.mode,
                "AppServerHost: control/setPermissionMode"
            );
            HandlerResult::ok_empty()
        }
        Err(error) => session_control_error(error),
    }
}

/// `control/applyPermissionUpdate` — apply one permission editor update.
pub(crate) async fn handle_apply_permission_update(
    params: coco_types::ApplyPermissionUpdateParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_controls::apply_permission_update(ctx.resolve_runtime().await, params.update)
        .await
    {
        Ok(()) => {
            info!("AppServerHost: control/applyPermissionUpdate");
            HandlerResult::ok_empty()
        }
        Err(error) => session_control_error(error),
    }
}

/// `control/resetSessionPermissionRules` — clear session-scoped allow/deny rules.
pub(crate) async fn handle_reset_session_permission_rules(ctx: &HandlerContext) -> HandlerResult {
    match session_controls::reset_permission_rules(ctx.resolve_runtime().await).await {
        Ok(result) => {
            info!(
                cleared_allow_rules = result.cleared_allow_rules,
                cleared_deny_rules = result.cleared_deny_rules,
                "AppServerHost: control/resetSessionPermissionRules"
            );
            HandlerResult::ok(coco_types::ResetSessionPermissionRulesResult {
                cleared_allow_rules: result.cleared_allow_rules,
                cleared_deny_rules: result.cleared_deny_rules,
            })
        }
        Err(error) => session_control_error(error),
    }
}
