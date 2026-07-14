use std::{sync::Arc, time::Duration};

use coco_app_server::{AppServer, ConnectionKey};
use coco_types::{SessionResumeResult, SurfaceId};

use crate::app_server_host::connection_runtime_binding::{
    build_connection_runtime_for_resume, configure_connection_mcp_bridge,
    install_app_server_session_runtime_state, touch_runtime_backed_resumed_session_activity,
};
use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};
use crate::app_session::AppSessionHandle;
use crate::session_resume::SessionResumeInput;

use super::session_loading::load_local_app_server_session_with_retrying_factory_parts;
use super::session_operation_error::SessionOperationError;
use super::session_registry::restore_session_seq_from_watermark;
use super::session_surfaces::attach_local_app_server_surface;

pub(crate) async fn load_app_server_resume_session(
    input: SessionResumeInput,
    state: &AppServerHostState,
) -> Result<crate::session_resume::LoadedResumeSession, SessionOperationError> {
    crate::session_resume::load_resume_session(state.session_manager_snapshot().await, input)
        .await
        .map_err(load_resume_session_error)
}

fn load_resume_session_error(
    error: crate::session_resume::LoadResumeSessionError,
) -> SessionOperationError {
    match &error {
        crate::session_resume::LoadResumeSessionError::InvalidRequest(_) => {
            SessionOperationError::invalid_request(error.message(), None)
        }
        crate::session_resume::LoadResumeSessionError::Internal(_) => {
            SessionOperationError::internal(error.message(), None)
        }
    }
}

pub(crate) async fn resume_app_server_session_with_runtime_replacement(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    connection: ConnectionKey,
    input: SessionResumeInput,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<SessionResumeResult, SessionOperationError> {
    let plan_mode_instructions = input.plan_mode_instructions.clone();
    let loaded = load_app_server_resume_session(input, &state).await?;
    let resumed_session_id = loaded.session_id.clone();
    let resumed_cwd = loaded.session.working_dir.clone();
    let prior_messages = loaded.conversation.messages.clone();
    if let Some(watermark) = loaded.session.session_seq_watermark {
        restore_session_seq_from_watermark(
            &app_server,
            &state,
            resumed_session_id.clone(),
            watermark,
        );
    }

    if let Some(handle) = app_server.registry().get(&resumed_session_id) {
        let runtime = handle.into_session();
        if !runtime
            .callback_requirements()
            .is_satisfied_by(&connection_profile)
        {
            return Err(SessionOperationError::invalid_request(
                "connection profile does not satisfy the live session callback requirements",
                Some(serde_json::json!({
                    "kind": "connection_profile_mismatch",
                    "session_id": resumed_session_id,
                })),
            ));
        }
        let surface_id =
            attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
        return build_session_resume_result(&loaded.session, surface_id);
    }

    let make_factory = || {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let session_id = resumed_session_id.clone();
        let cwd = resumed_cwd.clone();
        let prior_messages = prior_messages.clone();
        let connection_profile = Arc::clone(&connection_profile);
        let app_server = Arc::clone(&app_server);
        let plan_mode_instructions = plan_mode_instructions.clone();
        async move {
            let runtime = build_connection_runtime_for_resume(
                replacement,
                state,
                connection_profile,
                session_id.clone(),
                cwd,
                prior_messages,
                plan_mode_instructions,
                app_server,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<AppSessionHandle, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(
                runtime,
            ))
        }
    };

    let handle = load_local_app_server_session_with_retrying_factory_parts(
        &app_server,
        resumed_session_id.clone(),
        make_factory,
        turn_drain_timeout,
    )
    .await?;
    let runtime = handle.into_session();
    install_app_server_session_runtime_state(
        Arc::clone(&state),
        runtime.clone(),
        Arc::clone(&app_server),
    )
    .await;
    touch_runtime_backed_resumed_session_activity(&state, resumed_session_id.clone());

    let surface_id = attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
    configure_connection_mcp_bridge(&connection_profile, &runtime, Arc::clone(&app_server)).await;
    build_session_resume_result(&loaded.session, surface_id)
}

pub(crate) fn build_session_resume_result(
    session: &coco_session::Session,
    surface_id: SurfaceId,
) -> Result<SessionResumeResult, SessionOperationError> {
    let summary = crate::session_data::session_record_to_summary(session).map_err(|error| {
        SessionOperationError::internal(
            format!("session/resume returned invalid session id: {error}"),
            None,
        )
    })?;
    Ok(SessionResumeResult {
        session: summary,
        surface_id,
    })
}
