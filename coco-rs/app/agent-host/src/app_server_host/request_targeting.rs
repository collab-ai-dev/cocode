use std::sync::Arc;

use coco_app_server::{AppServer, JsonRpcDispatchError};
use coco_types::{ClientRequest, SessionId};

use crate::app_session::AppSessionHandle;

use super::SessionRequestContext;
use super::session_errors::app_server_lifecycle_error;

pub(crate) fn resolve_request_runtime(
    app_server: Option<&Arc<AppServer<AppSessionHandle>>>,
    connection: coco_app_server::ConnectionKey,
    request: &ClientRequest,
) -> Result<(Option<SessionId>, Option<SessionRequestContext>), JsonRpcDispatchError> {
    if let ClientRequest::SessionArchive(params) = request {
        let session_id = params.target.session_id().clone();
        let app_server = app_server.ok_or_else(|| JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/archive requires AppServer routing".to_string(),
            data: None,
        })?;
        let handle = match &params.target {
            coco_types::ArchiveTarget::Interactive(target) => {
                app_server
                    .validate_interactive_target(connection, target)
                    .map_err(|error| app_server_lifecycle_error("resolve archive target", error))?
                    .handle
            }
            coco_types::ArchiveTarget::Orphaned(_) => app_server
                .validate_orphan_archive_target(&session_id)
                .map_err(|error| {
                    app_server_lifecycle_error("validate orphan archive target", error)
                })?,
        };
        return Ok((
            Some(session_id.clone()),
            Some(SessionRequestContext {
                session_id,
                runtime: handle.runtime().clone(),
            }),
        ));
    }
    let Some(target) = request.interactive_target() else {
        let Some(target) = request.session_target() else {
            return Ok((None, None));
        };
        let runtime = app_server
            .and_then(|server| server.registry().get(&target.session_id))
            .map(AppSessionHandle::into_session);
        return Ok((
            Some(target.session_id.clone()),
            runtime.map(|runtime| SessionRequestContext {
                session_id: target.session_id.clone(),
                runtime,
            }),
        ));
    };
    let app_server = app_server.ok_or_else(|| JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: "interactive request requires AppServer routing".to_string(),
        data: None,
    })?;
    let validated = app_server
        .validate_interactive_target(connection, target)
        .map_err(|error| app_server_lifecycle_error("resolve request target", error))?;
    let runtime = validated.handle.runtime().clone();
    Ok((
        Some(target.session_id.clone()),
        Some(SessionRequestContext {
            session_id: target.session_id.clone(),
            runtime,
        }),
    ))
}
