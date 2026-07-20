use std::sync::Arc;

use coco_app_server::{AppServer, JsonRpcDispatchError};
use coco_types::{ClientRequest, RequestScope, SessionAccess, SessionId, request_scope};

use crate::app_session::AppSessionHandle;

use super::SessionRequestContext;
use super::session_errors::app_server_lifecycle_error;

pub(crate) fn resolve_request_runtime(
    app_server: Option<&Arc<AppServer<AppSessionHandle>>>,
    connection: coco_app_server::ConnectionKey,
    request: &ClientRequest,
) -> Result<(Option<SessionId>, Option<SessionRequestContext>), JsonRpcDispatchError> {
    let Some(target) = request.session_target() else {
        return Ok((None, None));
    };
    let Some(app_server) = app_server else {
        let session_id = target.session_id.clone();
        return Ok((Some(session_id), None));
    };
    let scope = request_scope(request.method());
    let handle = match scope {
        RequestScope::SessionFull => {
            app_server
                .validate_session_target(connection, target, SessionAccess::Full)
                .map_err(|error| app_server_lifecycle_error("resolve full session target", error))?
                .handle
        }
        RequestScope::SessionRead => {
            app_server
                .validate_session_target(connection, target, SessionAccess::ReadOnly)
                .map_err(|error| app_server_lifecycle_error("resolve read session target", error))?
                .handle
        }
        RequestScope::Lifecycle => {
            if matches!(request, ClientRequest::SessionClose(_)) {
                app_server
                    .validate_session_target(connection, target, SessionAccess::Full)
                    .map_err(|error| {
                        app_server_lifecycle_error("resolve lifecycle session target", error)
                    })?
                    .handle
            } else {
                let Some(handle) = app_server.registry().get(&target.session_id) else {
                    return Ok((Some(target.session_id.clone()), None));
                };
                handle
            }
        }
        RequestScope::Configuration => {
            let required_access = if matches!(request, ClientRequest::ConfigRead(_)) {
                SessionAccess::ReadOnly
            } else {
                SessionAccess::Full
            };
            app_server
                .validate_session_target(connection, target, required_access)
                .map_err(|error| {
                    app_server_lifecycle_error("resolve configuration session target", error)
                })?
                .handle
        }
        RequestScope::Connection | RequestScope::Process => {
            let Some(handle) = app_server.registry().get(&target.session_id) else {
                return Ok((Some(target.session_id.clone()), None));
            };
            handle
        }
    };
    let session_id = target.session_id.clone();
    Ok((
        Some(session_id.clone()),
        Some(SessionRequestContext {
            session_id,
            runtime: handle.runtime().clone(),
        }),
    ))
}
