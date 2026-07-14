use tracing::info;

use super::session_control_error;
use crate::app_server_host::outbound::send_session_event;
use crate::app_server_host::request_handlers::{
    DEFAULT_APP_SERVER_MODEL, HandlerContext, HandlerResult,
};
use crate::session_controls;

/// `control/setModel` — mutate the active session's model.
///
/// The updated model takes effect on the *next* `turn/start`. In-flight
/// turns continue running against the previous model (they'd need
/// restarting to swap models mid-call).
///
/// Passing `None` means "revert to the default model", which we
/// interpret as `claude-opus-4-6` (the AppServer lifecycle default).
pub(crate) async fn handle_set_model(
    params: coco_types::SetModelParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let new_model = params
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_APP_SERVER_MODEL.into());

    match session_controls::set_model(ctx.resolve_runtime().await, new_model).await {
        Ok(result) => {
            let session_id = result.session_id;
            info!(
                session_id = %session_id,
                old_model = %result.old_model,
                new_model = %result.new_model,
                "AppServerHost: control/setModel"
            );
            HandlerResult::ok_empty()
        }
        Err(error) => session_control_error(error),
    }
}

/// `control/setModelRole` — apply an in-memory role/provider/model override.
pub(crate) async fn handle_set_model_role(
    params: coco_types::SetModelRoleParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let result = match session_controls::set_model_role(
        ctx.resolve_runtime().await,
        params.role,
        params.provider,
        params.model_id,
        params.effort,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return session_control_error(error),
    };
    info!(
        role = %result.changed.role.as_str(),
        provider = %result.changed.provider,
        model_id = %result.changed.model_id,
        effort = ?result.changed.effort,
        "AppServerHost: control/setModelRole"
    );

    let _ = send_session_event(
        &ctx.notif_tx,
        result.session_id,
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::ModelRoleChanged(
            result.changed.clone(),
        )),
    )
    .await;
    HandlerResult::ok(coco_types::SetModelRoleResult {
        changed: result.changed,
        display_name: result.display_name,
    })
}

/// `control/setThinking` — mutate the session's thinking level.
///
/// `thinking_level = None` clears the override so turns fall back to
/// the engine's default.
pub(crate) async fn handle_set_thinking(
    params: coco_types::SetThinkingParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let thinking_level = params.thinking_level;
    let result = match session_controls::set_thinking(
        ctx.resolve_runtime().await,
        ctx.active_session_id().await,
        thinking_level.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return session_control_error(error),
    };
    info!(
        session_id = %result.session_id,
        level = ?thinking_level,
        "AppServerHost: control/setThinking"
    );
    if let Some(changed) = result.changed {
        let _ = send_session_event(
            &ctx.notif_tx,
            result.session_id,
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::ModelRoleChanged(
                changed,
            )),
        )
        .await;
    }
    HandlerResult::ok_empty()
}

/// `control/setAgentColor` — mutate the session's UI badge color.
pub(crate) async fn handle_set_agent_color(
    params: coco_types::SetAgentColorParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_controls::set_agent_color(ctx.resolve_runtime().await, params.color).await {
        Ok(()) => {
            info!("AppServerHost: control/setAgentColor");
            HandlerResult::ok_empty()
        }
        Err(error) => session_control_error(error),
    }
}
