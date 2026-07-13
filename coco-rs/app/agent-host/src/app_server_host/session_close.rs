use std::{sync::Arc, time::Duration};

use coco_app_server::{AppCloseStart, AppServer, JsonRpcDispatchError};
use coco_types::SessionId;
use tracing::debug;

use crate::app_session::AppSessionHandle;

use super::AppServerHostState;
use super::session_errors::{LifecycleError, local_lifecycle_error, local_lifecycle_error_parts};

pub(crate) async fn close_local_app_server_session(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    close_local_app_server_session_parts(app_server, state, session_id, turn_drain_timeout)
        .await
        .map_err(LifecycleError::into_dispatch_error)
}

pub(crate) async fn close_local_app_server_session_parts(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<(), LifecycleError> {
    if !app_server
        .list_live_sessions()
        .iter()
        .any(|summary| summary.session_id == session_id)
    {
        return Ok(());
    }
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_close(session_id, move |handle| async move {
            close_app_server_session_state(&close_state, handle.session_id()).await;
            close_local_session_handle(handle, turn_drain_timeout).await;
        })
        .map_err(|error| local_lifecycle_error_parts("archive session", error))?
    {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("archive session", error))
}

pub(crate) async fn close_orphan_local_app_server_session_parts(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<(), LifecycleError> {
    let close_state = Arc::clone(&state);
    let mut completion = app_server
        .spawn_close_orphan(session_id, move |handle| async move {
            close_app_server_session_state(&close_state, handle.session_id()).await;
            close_local_session_handle(handle, turn_drain_timeout).await;
        })
        .map_err(|error| local_lifecycle_error_parts("archive orphan session", error))?
        .completion();
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("archive orphan session", error))
}

pub async fn shutdown_local_app_server_sessions(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    let close_state = Arc::clone(&state);
    let shutdown = app_server.spawn_shutdown(move |handle| {
        let close_state = Arc::clone(&close_state);
        async move {
            close_app_server_session_state(&close_state, handle.session_id()).await;
            close_local_session_handle(handle, turn_drain_timeout).await;
        }
    });

    let mut first_error = shutdown
        .errors
        .into_iter()
        .next()
        .map(|(_, error)| local_lifecycle_error("shutdown sessions", error));
    for session in shutdown.sessions {
        let mut completion = session.completion;
        if let Err(error) = completion.wait().await
            && first_error.is_none()
        {
            first_error = Some(local_lifecycle_error("shutdown sessions", error));
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) async fn close_local_session_handle(
    handle: AppSessionHandle,
    turn_drain_timeout: Duration,
) {
    close_local_session_handle_with_reason(
        handle,
        coco_hooks::orchestration::ExitReason::Other,
        turn_drain_timeout,
    )
    .await;
}

pub(crate) async fn close_local_session_handle_with_reason(
    handle: AppSessionHandle,
    reason: coco_hooks::orchestration::ExitReason,
    turn_drain_timeout: Duration,
) {
    if let Some(current_session_id) = handle
        .runtime()
        .close_if_current_session(handle.session_id(), reason, turn_drain_timeout)
        .await
    {
        debug!(
            target: "coco::app_server_local",
            registry_session_id = %handle.session_id(),
            current_session_id = %current_session_id,
            "skipping local AppServer close cascade for stale registry snapshot"
        );
        return;
    }
    debug!(
        target: "coco::app_server_local",
        session_id = %handle.session_id(),
        "local AppServer close cascade completed fused runtime boundary"
    );
}

pub(crate) async fn close_app_server_session_state(
    state: &AppServerHostState,
    session_id: &SessionId,
) {
    persist_session_seq_watermark_on_close(state, session_id).await;
    state.forget_session_activity(session_id);
}

/// Persist the exact `session_seq` high-water mark before a session closes so a
/// later resume skips ahead from the true final value rather than a stale
/// interval watermark. Awaited (not best-effort) so a clean shutdown always
/// records an exact anchor.
pub(crate) async fn persist_session_seq_watermark_on_close(
    state: &AppServerHostState,
    session_id: &SessionId,
) {
    let Some(high_water) = state.session_seq_allocator().high_water(session_id) else {
        return;
    };
    let Some(manager) = state.session_manager_snapshot().await else {
        return;
    };
    let id = session_id.as_str().to_string();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(error) = manager.persist_session_seq_watermark(&id, high_water) {
            tracing::debug!(session_id = %id, %error, "failed to persist session_seq watermark at close");
        }
    })
    .await;
}
