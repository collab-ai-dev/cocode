use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use coco_app_server::{AppLoadStart, AppServer, JsonRpcDispatchError};
use coco_types::SessionId;

use crate::app_session::AppSessionHandle;

use super::session_errors::{LifecycleError, local_lifecycle_error_parts};

pub(crate) async fn load_local_app_server_session_with_factory<F>(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    factory: F,
) -> Result<AppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    load_local_app_server_session_with_factory_parts(app_server, session_id, factory)
        .await
        .map_err(LifecycleError::into_dispatch_error)
}

pub(crate) async fn load_local_app_server_session_with_factory_parts<F>(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    factory: F,
) -> Result<AppSessionHandle, LifecycleError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let mut completion = match app_server
        .spawn_load(session_id.clone(), factory)
        .map_err(|error| local_lifecycle_error_parts("load session", error))?
    {
        AppLoadStart::Started { completion } | AppLoadStart::Loading(completion) => completion,
        AppLoadStart::Live(handle) => return Ok(handle),
        AppLoadStart::Closing(_) => {
            return Err(LifecycleError::Internal {
                message: format!("local AppServer session {session_id} is closing"),
                data: None,
            });
        }
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("load session", error))
}

/// New-only load owner for `session/start`.
///
/// Start must mint or claim a fresh identity: the only valid load outcome is a
/// freshly reserved `Started` slot. A `Loading`, `Live`, or `Closing` slot for
/// this id means another owner already holds it, so start rejects it instead of
/// reusing or mutating that runtime. The AppServer load invariant drops our
/// factory future unpolled for those outcomes, so rejection leaves the existing
/// session and the registry untouched.
pub(crate) async fn load_local_app_server_session_new_only<F>(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    factory: F,
) -> Result<AppSessionHandle, LifecycleError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let mut completion = match app_server
        .spawn_load(session_id.clone(), factory)
        .map_err(|error| local_lifecycle_error_parts("start session", error))?
    {
        AppLoadStart::Started { completion } => completion,
        AppLoadStart::Loading(_) => return Err(start_slot_conflict(&session_id, "loading")),
        AppLoadStart::Live(_) => return Err(start_slot_conflict(&session_id, "live")),
        AppLoadStart::Closing(_) => return Err(start_slot_conflict(&session_id, "closing")),
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("start session", error))
}

fn start_slot_conflict(session_id: &SessionId, state: &str) -> LifecycleError {
    LifecycleError::InvalidRequest {
        message: format!(
            "session/start requires a new session id; {session_id} is already {state}"
        ),
        data: Some(serde_json::json!({
            "kind": "session_start_slot_conflict",
            "session_id": session_id,
            "state": state,
        })),
    }
}

pub(crate) async fn load_local_app_server_session_with_retrying_factory_parts<Make, F>(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    make_factory: Make,
    close_wait_timeout: Duration,
) -> Result<AppSessionHandle, LifecycleError>
where
    Make: Fn() -> F,
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    loop {
        match app_server
            .spawn_load(session_id.clone(), make_factory())
            .map_err(|error| local_lifecycle_error_parts("load session", error))?
        {
            AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
                return completion
                    .wait()
                    .await
                    .map_err(|error| local_lifecycle_error_parts("load session", error));
            }
            AppLoadStart::Live(handle) => return Ok(handle),
            AppLoadStart::Closing(mut completion) => {
                tokio::time::timeout(close_wait_timeout, completion.wait())
                    .await
                    .map_err(|_| LifecycleError::Internal {
                        message: format!(
                            "timed out waiting for closing session {session_id} before resume"
                        ),
                        data: Some(serde_json::json!({
                            "kind": "session_close_timeout",
                            "session_id": session_id,
                        })),
                    })?
                    .map_err(|error| {
                        local_lifecycle_error_parts("wait for closing session", error)
                    })?;
            }
        }
    }
}

pub async fn load_local_app_server_session_runtime(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
) -> Result<AppSessionHandle, JsonRpcDispatchError> {
    let build_session_id = session_id.clone();
    load_local_app_server_session_with_factory(app_server, session_id, async move {
        let runtime = runtime_factory
            .build_with_session_id(build_session_id, Default::default())
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
        Ok(AppSessionHandle::from_runtime(runtime))
    })
    .await
}

pub async fn load_local_app_server_session_runtime_with_cwd(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    session_id: SessionId,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    cwd: PathBuf,
) -> Result<AppSessionHandle, JsonRpcDispatchError> {
    let build_session_id = session_id.clone();
    load_local_app_server_session_with_factory(app_server, session_id, async move {
        let runtime = runtime_factory
            .build_with_session_id_and_cwd(build_session_id, cwd, Default::default())
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
        Ok(AppSessionHandle::from_runtime(runtime))
    })
    .await
}
