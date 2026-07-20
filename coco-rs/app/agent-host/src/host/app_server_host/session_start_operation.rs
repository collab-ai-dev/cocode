use std::sync::Arc;

use coco_app_server::{AppServer, ConnectionKey};
use coco_types::SessionStartResult;
use tracing::info;

use crate::app_server_host::connection_runtime_binding::{
    build_connection_runtime_for_start, configure_connection_mcp_bridge,
    install_app_server_session_runtime_state, register_connection_callback_owners,
};
use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};
use crate::app_session::AppSessionHandle;
use crate::session_start::SessionStartInput;

use super::request_handlers::DEFAULT_APP_SERVER_MODEL;
use super::session_connections::attach_local_app_server_session;
use super::session_loading::load_local_app_server_session_new_only;
use super::session_operation_error::SessionOperationError;

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

fn touch_started_session_activity(
    state: &AppServerHostState,
    prepared: &crate::session_start::PreparedStartSession,
) {
    // Runtime configuration (model/permission/accounting) is now applied inside
    // the load factory on the unpublished runtime (CS-1 §0.1 item 5); the
    // post-promote path only records activity.
    state.touch_session_activity(prepared.session_id.clone());
}

pub(crate) async fn start_app_server_session_with_runtime_replacement(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    connection: ConnectionKey,
    input: SessionStartInput,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
) -> Result<SessionStartResult, SessionOperationError> {
    let prepared = prepare_app_server_session_start(input, &state, &connection_profile).await?;
    let started_session_id = prepared.session_id.clone();

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

    let handle =
        load_local_app_server_session_new_only(&app_server, started_session_id.clone(), factory)
            .await?;
    let runtime = handle.into_session();

    install_app_server_session_runtime_state(
        Arc::clone(&state),
        runtime.clone(),
        Arc::clone(&app_server),
    )
    .await;
    touch_started_session_activity(&state, &prepared);

    attach_local_app_server_session(&app_server, connection, started_session_id.clone())?;
    register_connection_callback_owners(&connection_profile, &runtime, &app_server, connection)
        .map_err(|error| {
            SessionOperationError::internal(
                format!("register session/start callback owners: {error}"),
                Some(serde_json::json!({ "kind": "callback_owner_registration_failed" })),
            )
        })?;
    runtime
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Startup)
        .await;
    configure_connection_mcp_bridge(
        &connection_profile,
        &runtime,
        Arc::clone(&app_server),
        connection,
    )
    .await;
    Ok(SessionStartResult {
        session_id: started_session_id,
    })
}
