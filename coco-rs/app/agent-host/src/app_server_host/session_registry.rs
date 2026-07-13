use std::{future::Future, sync::Arc, time::Duration};

use coco_app_server::{AppLoadStart, AppServer, JsonRpcDispatchError};
use coco_types::{SessionId, SurfaceId};

use super::AppServerHostState;
use crate::app_session::AppSessionHandle;

use super::{
    session_close::{
        close_app_server_session_state, close_local_session_handle,
        close_local_session_handle_with_reason,
    },
    session_errors::{
        LifecycleError, app_server_lifecycle_error, app_server_lifecycle_error_parts,
        local_lifecycle_error, local_lifecycle_error_parts,
    },
    session_surfaces::local_replace_calling_surface,
};

pub(crate) fn restore_session_seq_from_watermark(
    app_server: &AppServer<AppSessionHandle>,
    state: &AppServerHostState,
    session_id: SessionId,
    watermark: i64,
) {
    state
        .session_seq_allocator()
        .initialize_after_watermark(&session_id, watermark);
    app_server.initialize_session_ring_watermark(session_id, watermark);
}

pub(crate) async fn replace_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<Option<(AppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    replace_local_app_server_session_with_factory_and_close_reason(
        app_server,
        state,
        old_session_id,
        new_session_id,
        factory,
        coco_hooks::orchestration::ExitReason::Other,
        turn_drain_timeout,
    )
    .await
}

pub(crate) async fn replace_local_app_server_session_with_factory_and_close_reason<F>(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    close_reason: coco_hooks::orchestration::ExitReason,
    turn_drain_timeout: Duration,
) -> Result<Option<(AppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let Some(calling_surface) = local_replace_calling_surface(&app_server, &old_session_id) else {
        return Ok(None);
    };
    let calling_surface_id = calling_surface.clone();
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace(
            old_session_id,
            new_session_id,
            calling_surface,
            factory,
            move |handle| async move {
                close_app_server_session_state(&close_state, handle.session_id()).await;
                close_local_session_handle_with_reason(handle, close_reason, turn_drain_timeout)
                    .await;
            },
        )
        .map_err(|error| app_server_lifecycle_error("replace session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map(|handle| Some((handle, calling_surface_id)))
        .map_err(|error| local_lifecycle_error("replace session", error))
}

pub(crate) async fn replace_detached_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<AppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    replace_detached_local_app_server_session_with_factory_parts(
        app_server,
        state,
        old_session_id,
        new_session_id,
        factory,
        turn_drain_timeout,
    )
    .await
    .map_err(LifecycleError::into_dispatch_error)
}

pub(crate) async fn replace_detached_local_app_server_session_with_factory_parts<F>(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<AppSessionHandle, LifecycleError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace_detached(
            old_session_id,
            new_session_id,
            factory,
            move |handle| async move {
                close_app_server_session_state(&close_state, handle.session_id()).await;
                close_local_session_handle(handle, turn_drain_timeout).await;
            },
        )
        .map_err(|error| app_server_lifecycle_error_parts("replace detached session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("replace detached session", error))
}

pub(crate) async fn register_local_app_server_session(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    handle: AppSessionHandle,
) -> Result<(), JsonRpcDispatchError> {
    let session_id = handle.session_id().clone();
    let handle_for_load = handle.clone();
    match app_server
        .spawn_load(session_id, async {
            Ok::<AppSessionHandle, coco_app_server::RegistryError>(handle_for_load)
        })
        .map_err(|error| local_lifecycle_error("register session", error))?
    {
        AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
            completion
                .wait()
                .await
                .map(|_| ())
                .map_err(|error| local_lifecycle_error("register session", error))
        }
        AppLoadStart::Live(_) => {
            let refresh_session_id = handle.session_id().clone();
            app_server
                .registry()
                .replace_live_handle(&refresh_session_id, handle)
                .map_err(|error| local_lifecycle_error("refresh live session", error))?;
            Ok(())
        }
        AppLoadStart::Closing(_) => Ok(()),
    }
}
