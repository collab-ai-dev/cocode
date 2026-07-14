use tracing::info;

use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_data;
use crate::session_data::PersistedSessionDataError;

/// `session/list` — enumerate persisted sessions, newest first.
///
/// Delegates to `SessionManager::list()`. Returns an empty list if no
/// manager is wired (session persistence disabled).
///
/// Errors:
/// - `INTERNAL_ERROR` if `SessionManager::list()` fails (e.g. filesystem error)
pub(crate) async fn handle_session_list(ctx: &HandlerContext) -> HandlerResult {
    let manager = ctx.state.session_manager_snapshot().await;
    if manager.is_none() {
        info!("AppServerHost: session/list (no session manager installed, returning empty)");
    }
    match session_data::persisted_session_list(manager).await {
        Ok(result) => {
            info!(count = result.sessions.len(), "AppServerHost: session/list");
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
pub(crate) async fn handle_session_read(
    params: coco_types::SessionReadParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_data::persisted_session_read(ctx.state.session_manager_snapshot().await, &params)
        .await
    {
        Ok(result) => {
            info!(session_id = %params.target.session_id, "AppServerHost: session/read");
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
pub(crate) async fn handle_session_turns_list(
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
            info!(session_id = %params.target.session_id, "AppServerHost: session/turns/list");
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
