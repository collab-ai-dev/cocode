use std::{sync::Arc, time::Duration};

use coco_app_server::{AppServer, ConnectionKey};
use coco_types::SessionStartResult;
use tracing::info;

use crate::app_server_host::connection_runtime_binding::{
    build_connection_runtime_for_start, configure_connection_mcp_bridge,
    install_app_server_session_runtime_state,
};
use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};
use crate::app_session::AppSessionHandle;
use crate::session_start::SessionStartInput;

use super::request_handlers::DEFAULT_APP_SERVER_MODEL;
use super::session_loading::load_local_app_server_session_with_factory_parts;
use super::session_operation_error::SessionOperationError;
use super::session_registry::replace_detached_local_app_server_session_with_factory_parts;
use super::session_surfaces::{attach_local_app_server_surface, registered_detached_session};

pub(crate) async fn prepare_app_server_session_start(
    input: SessionStartInput,
    state: &AppServerHostState,
    connection_profile: &coco_types::ConnectionProfile,
) -> Result<crate::session_start::PreparedStartSession, SessionOperationError> {
    let workspace_cwd = if input.cwd.is_some() {
        None
    } else {
        state.workspace_cwd().await.ok()
    };
    let prepared = crate::session_start::prepare_session_start(
        input,
        workspace_cwd,
        DEFAULT_APP_SERVER_MODEL,
        connection_profile,
    )
    .map_err(prepare_session_start_error)?;
    info!(
        session_id = %prepared.session_id,
        cwd = %prepared.cwd,
        model = %prepared.model,
        "AppServerHost: session/start"
    );
    Ok(prepared)
}

fn prepare_session_start_error(
    error: crate::session_start::PrepareSessionStartError,
) -> SessionOperationError {
    SessionOperationError::invalid_request(error.message(), None)
}

async fn install_scoped_started_session_state(
    state: &AppServerHostState,
    prepared: &crate::session_start::PreparedStartSession,
    runtime: &crate::session_runtime::SessionHandle,
) {
    crate::session_start::apply_prepared_session_start(prepared, runtime).await;
    state.touch_session_activity(prepared.session_id.clone());
}

pub(crate) async fn start_app_server_session_with_runtime_replacement(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    connection: ConnectionKey,
    input: SessionStartInput,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<SessionStartResult, SessionOperationError> {
    let prepared = prepare_app_server_session_start(input, &state, &connection_profile).await?;
    let started_session_id = prepared.session_id.clone();
    let startup_session_id = replacement.startup_session_id.clone();

    let factory = {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let prepared = prepared.clone();
        let connection_profile = Arc::clone(&connection_profile);
        let app_server = Arc::clone(&app_server);
        async move {
            let runtime = build_connection_runtime_for_start(
                replacement,
                state,
                connection_profile,
                prepared,
                app_server,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<AppSessionHandle, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(
                runtime,
            ))
        }
    };

    // The bootstrap runtime is an implementation placeholder, not a client
    // session. Replace that one explicit surfaceless slot on the first start;
    // after it is gone, every later start creates a new slot and never closes
    // another user's session.
    let handle =
        if registered_detached_session(&app_server, &startup_session_id, &started_session_id) {
            replace_detached_local_app_server_session_with_factory_parts(
                Arc::clone(&app_server),
                Arc::clone(&state),
                startup_session_id,
                started_session_id.clone(),
                factory,
                turn_drain_timeout,
            )
            .await?
        } else {
            load_local_app_server_session_with_factory_parts(
                &app_server,
                started_session_id.clone(),
                factory,
            )
            .await?
        };
    let runtime = handle.into_session();

    install_app_server_session_runtime_state(
        Arc::clone(&state),
        runtime.clone(),
        Arc::clone(&app_server),
    )
    .await;
    install_scoped_started_session_state(&state, &prepared, &runtime).await;

    let surface_id =
        attach_local_app_server_surface(&app_server, connection, started_session_id.clone())?;
    configure_connection_mcp_bridge(&connection_profile, &runtime, Arc::clone(&app_server)).await;
    Ok(SessionStartResult {
        session_id: started_session_id,
        surface_id: Some(surface_id),
    })
}
