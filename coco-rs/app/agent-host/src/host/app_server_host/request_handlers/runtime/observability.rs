use tracing::info;

use super::session_control_error;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_controls;

/// `session/cost` — return the active session's live usage/cost report.
pub(crate) async fn handle_session_cost(ctx: &HandlerContext) -> HandlerResult {
    match session_controls::session_cost(ctx.resolve_runtime().await).await {
        Ok(result) => {
            info!("AppServerHost: session/cost");
            HandlerResult::ok(result)
        }
        Err(error) => session_control_error(error),
    }
}

/// `session/status` — return the active session's live status report.
pub(crate) async fn handle_session_status(ctx: &HandlerContext) -> HandlerResult {
    match session_controls::session_status(ctx.resolve_runtime().await).await {
        Ok(result) => {
            info!("AppServerHost: session/status");
            HandlerResult::ok(result)
        }
        Err(error) => session_control_error(error),
    }
}

/// `context/usage` — return the active session's current Main context view.
pub(crate) async fn handle_context_usage(ctx: &HandlerContext) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let has_active_session = runtime.is_some() || ctx.active_session_id().await.is_some();
    match session_controls::context_usage(runtime, has_active_session).await {
        Ok(result) => HandlerResult::ok(result),
        Err(error) => session_control_error(error),
    }
}
