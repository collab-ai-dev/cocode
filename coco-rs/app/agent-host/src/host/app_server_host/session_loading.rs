use std::{future::Future, sync::Arc, time::Duration};

use coco_app_server::{AppLoadStart, AppServer};
use coco_types::SessionId;

use crate::app_session::AppSessionHandle;

use super::AppServerHostState;
use super::session_close::runtime_load_teardown;
use super::session_errors::{LifecycleError, local_lifecycle_error_parts};

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
    state: Arc<AppServerHostState>,
    session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<AppSessionHandle, LifecycleError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let mut completion = match app_server
        .spawn_load(
            session_id.clone(),
            factory,
            runtime_load_teardown(state, turn_drain_timeout),
        )
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
    state: Arc<AppServerHostState>,
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
            .spawn_load(
                session_id.clone(),
                make_factory(),
                runtime_load_teardown(Arc::clone(&state), close_wait_timeout),
            )
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
