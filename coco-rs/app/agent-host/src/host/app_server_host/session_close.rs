use std::{sync::Arc, time::Duration};

use coco_app_server::{AppCloseStart, AppServer, JsonRpcDispatchError, RegistryError};
use coco_types::CoreEvent;
use coco_types::SessionId;
use tokio::sync::mpsc;
use tracing::debug;

use crate::app_session::AppSessionHandle;

use super::AppServerHostState;
use super::OutboundMessage;
use super::outbound::send_session_event_and_wait;
use super::session_errors::{
    LifecycleError, app_server_lifecycle_error_parts, local_lifecycle_error,
    registry_lifecycle_error_parts,
};

/// Standard teardown for a runtime whose load commit failed: run the same
/// state-cleanup + close cascade a normal close would, so the constructed
/// runtime is never silently dropped (SessionEnd hooks must fire and its
/// session tasks must join).
pub(crate) fn runtime_load_teardown(
    state: Arc<AppServerHostState>,
    turn_drain_timeout: Duration,
) -> impl FnOnce(
    AppSessionHandle,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), RegistryError>> + Send>,
> + Send
+ 'static {
    move |handle| {
        Box::pin(async move {
            close_app_server_session_state(&state, handle.session_id()).await;
            close_local_session_handle(handle, turn_drain_timeout).await
        })
    }
}

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
    if !app_server.has_session_slot(&session_id) {
        return Ok(());
    }
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_close(session_id, move |handle| {
            let close_state = Arc::clone(&close_state);
            async move {
                close_app_server_session_state(&close_state, handle.session_id()).await;
                close_local_session_handle(handle, turn_drain_timeout).await
            }
        })
        .map_err(|error| app_server_lifecycle_error_parts("close session", error))?
    {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| registry_lifecycle_error_parts("close session", error))
}

/// Idle-supervisor close: commits `Live -> Closing` only while the session
/// has zero attached connections (checked atomically against concurrent
/// attaches inside the registry lock). Returns `Ok(false)` when the session
/// was gone or no longer idle — the supervisor just skips it.
pub(crate) async fn close_local_app_server_session_if_unattached(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<bool, LifecycleError> {
    if !app_server.has_session_slot(&session_id) {
        return Ok(false);
    }
    let close_state = Arc::clone(&state);
    let start = match app_server.spawn_close_when_unattached(session_id, move |handle| {
        let close_state = Arc::clone(&close_state);
        async move {
            close_app_server_session_state(&close_state, handle.session_id()).await;
            close_local_session_handle(handle, turn_drain_timeout).await
        }
    }) {
        Ok(start) => start,
        Err(coco_app_server::AppServerError::Registry { ref source, .. })
            if matches!(source, RegistryError::CloseAborted { .. }) =>
        {
            return Ok(false);
        }
        Err(error) => {
            return Err(app_server_lifecycle_error_parts(
                "close idle session",
                error,
            ));
        }
    };
    let mut completion = match start {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map(|()| true)
        .map_err(|error| registry_lifecycle_error_parts("close idle session", error))
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
            close_local_session_handle(handle, turn_drain_timeout).await
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

    // Abort and join any lifecycle owner tasks still in flight so process
    // shutdown does not leave them detached past the deadline (CS-3c).
    app_server.abort_and_join_owner_tasks().await;

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) async fn close_local_app_server_session_and_emit_result(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
    notif_tx: mpsc::Sender<OutboundMessage>,
) -> Result<(), LifecycleError> {
    close_app_server_session_with_callback(
        app_server,
        state,
        session_id,
        turn_drain_timeout,
        notif_tx,
        "close session",
    )
    .await
}

async fn close_app_server_session_with_callback(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
    notif_tx: mpsc::Sender<OutboundMessage>,
    operation: &'static str,
) -> Result<(), LifecycleError> {
    let close_state = Arc::clone(&state);
    let close_notif_tx = notif_tx.clone();
    let start = app_server
        .spawn_close(session_id, move |handle| {
            let close_state = Arc::clone(&close_state);
            let close_notif_tx = close_notif_tx.clone();
            async move {
                close_app_server_session_state(&close_state, handle.session_id()).await;
                close_local_session_handle(handle.clone(), turn_drain_timeout).await?;
                emit_final_session_result(&close_notif_tx, &handle).await;
                Ok(())
            }
        })
        .map_err(|error| app_server_lifecycle_error_parts(operation, error))?;

    let mut completion = match start {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| registry_lifecycle_error_parts(operation, error))
}

async fn emit_final_session_result(
    notif_tx: &mpsc::Sender<OutboundMessage>,
    handle: &AppSessionHandle,
) {
    let result = crate::session_close::build_session_result(handle.runtime(), "closed");
    let event = CoreEvent::Protocol(coco_types::ServerNotification::SessionResult(Box::new(
        result,
    )));
    let _ = send_session_event_and_wait(notif_tx, handle.session_id().clone(), event).await;
}

pub(crate) async fn close_local_session_handle(
    handle: AppSessionHandle,
    turn_drain_timeout: Duration,
) -> Result<(), RegistryError> {
    close_local_session_handle_with_reason(
        handle,
        coco_hooks::orchestration::ExitReason::Other,
        turn_drain_timeout,
    )
    .await
}

pub(crate) async fn close_local_session_handle_with_reason(
    handle: AppSessionHandle,
    reason: coco_hooks::orchestration::ExitReason,
    turn_drain_timeout: Duration,
) -> Result<(), RegistryError> {
    handle
        .runtime()
        .close_runtime(reason, turn_drain_timeout)
        .await
        .map_err(|error| close_drain_error(handle.session_id(), error))?;
    debug!(
        target: "coco::app_server_local",
        session_id = %handle.session_id(),
        "local AppServer close cascade completed fused runtime boundary"
    );
    Ok(())
}

fn close_drain_error(
    session_id: &SessionId,
    error: crate::session_runtime::SessionCloseDrainError,
) -> RegistryError {
    let timeout_ms = error.timeout().as_millis().min(i64::MAX as u128) as i64;
    RegistryError::close_failed_with_data(
        format!("session {session_id} close timed out: {error}"),
        Some(serde_json::json!({
            "kind": "session_close_timeout",
            "session_id": session_id,
            "task": error.task(),
            "timeout_ms": timeout_ms,
        })),
    )
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
