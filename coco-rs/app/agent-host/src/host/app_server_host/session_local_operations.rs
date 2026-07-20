use std::{sync::Arc, time::Duration};

use coco_app_server::AppServer;
use tokio::sync::mpsc;

use crate::app_server_host::AppServerHostState;
use crate::app_server_host::OutboundMessage;
use crate::app_session::AppSessionHandle;

use super::session_close::close_local_app_server_session_and_emit_result;
use super::session_errors::app_server_lifecycle_error_parts;
use super::session_operation_error::SessionOperationError;
use super::session_operation_input::LocalSessionOperation;

pub(crate) async fn apply_local_session_operation(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    request: LocalSessionOperation,
    turn_drain_timeout: Duration,
    notif_tx: mpsc::Sender<OutboundMessage>,
) -> Result<(), SessionOperationError> {
    match request {
        LocalSessionOperation::Close { connection, target } => {
            let session_id = target.session_id.clone();
            app_server
                .validate_session_target(connection, &target, coco_types::SessionAccess::Full)
                .map_err(|error| {
                    app_server_lifecycle_error_parts("validate close target", error)
                })?;
            close_local_app_server_session_and_emit_result(
                app_server,
                state,
                session_id,
                turn_drain_timeout,
                notif_tx,
            )
            .await?;
            Ok(())
        }
    }
}
