use std::{future::Future, sync::Arc, time::Duration};

use coco_app_server::{AppLoadStart, AppServer, JsonRpcDispatchError};
use coco_types::SessionId;

use super::AppServerHostState;
use crate::app_session::AppSessionHandle;

use super::session_close::runtime_load_teardown;
use super::session_errors::local_lifecycle_error;

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

pub(crate) async fn register_local_app_server_session(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    handle: AppSessionHandle,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    let session_id = handle.session_id().clone();
    let handle_for_load = handle.clone();
    match app_server
        .spawn_load(
            session_id,
            async { Ok::<AppSessionHandle, coco_app_server::RegistryError>(handle_for_load) },
            runtime_load_teardown(state, turn_drain_timeout),
        )
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

/// Reserve and construct an ephemeral sidechat child under a live `parent`.
///
/// The registry reservation happens synchronously before `factory` is polled,
/// so parent close/replace and competing child creation cannot race an
/// unowned, fully-built runtime into existence.
pub(crate) async fn load_local_app_server_child_session<F>(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    parent: SessionId,
    child_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<AppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    match app_server
        .spawn_child_load(
            parent,
            child_id,
            factory,
            runtime_load_teardown(state, turn_drain_timeout),
        )
        .map_err(|error| local_lifecycle_error("load sidechat child", error))?
    {
        AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
            completion
                .wait()
                .await
                .map_err(|error| local_lifecycle_error("load sidechat child", error))
        }
        AppLoadStart::Live(handle) => Ok(handle),
        AppLoadStart::Closing(_) => Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "sidechat child is closing".to_string(),
            data: Some(serde_json::json!({ "kind": "sidechat_child_closing" })),
        }),
    }
}
