use std::sync::Arc;

use coco_app_server::{AppLoadStart, AppServer, JsonRpcDispatchError};
use coco_types::SessionId;

use super::AppServerHostState;
use crate::app_session::AppSessionHandle;

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
