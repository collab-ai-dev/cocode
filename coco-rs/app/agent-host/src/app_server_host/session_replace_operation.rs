use std::{sync::Arc, time::Duration};

use coco_app_server::{AppServer, ConnectionKey};
use coco_types::SessionReplaceResult;

use crate::app_server_host::connection_runtime_binding::{
    build_connection_runtime_for_resume, build_connection_runtime_for_start,
    configure_connection_mcp_bridge, install_app_server_sandbox_reload_subscription,
};
use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};
use crate::app_session::AppSessionHandle;
use crate::session_resume::SessionResumeInput;

use super::session_close::{
    close_app_server_session_state, close_local_app_server_session_parts,
    close_local_session_handle_with_reason,
};
use super::session_errors::app_server_lifecycle_error_parts;
use super::session_loading::{
    load_local_app_server_session_with_factory_parts,
    load_local_app_server_session_with_retrying_factory_parts,
};
use super::session_operation_error::SessionOperationError;
use super::session_operation_input::{SessionReplaceDestination, SessionReplaceInput};
use super::session_registry::restore_session_seq_from_watermark;
use super::session_resume_operation::load_app_server_resume_session;
use super::session_start_operation::prepare_app_server_session_start;

pub(crate) async fn replace_app_server_session_with_runtime(
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    connection: ConnectionKey,
    input: SessionReplaceInput,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<SessionReplaceResult, SessionOperationError> {
    app_server
        .validate_interactive_target(connection, &input.source)
        .map_err(|error| app_server_lifecycle_error_parts("validate replacement source", error))?;
    let source_session_id = input.source.session_id.clone();
    let source_surface_id = input.source.surface_id.clone();

    let (destination_id, destination_handle, configure_profile) = match input.destination {
        SessionReplaceDestination::Fresh(start_input) => {
            let prepared =
                prepare_app_server_session_start(start_input, &state, &connection_profile).await?;
            let destination_id = prepared.session_id.clone();
            let factory = {
                let state = Arc::clone(&state);
                let replacement = replacement.clone();
                let profile = Arc::clone(&connection_profile);
                let app_server = Arc::clone(&app_server);
                async move {
                    let runtime = build_connection_runtime_for_start(
                        replacement,
                        state,
                        profile,
                        prepared,
                        app_server,
                    )
                    .await
                    .map_err(|error| {
                        coco_app_server::RegistryError::load_failed(error.to_string())
                    })?;
                    Ok::<_, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(runtime))
                }
            };
            let handle = load_local_app_server_session_with_factory_parts(
                &app_server,
                destination_id.clone(),
                factory,
            )
            .await?;
            (destination_id, handle, true)
        }
        SessionReplaceDestination::Resume(target) => {
            if target.session_id == source_session_id {
                return Err(SessionOperationError::invalid_params(
                    "session/replace destination must differ from its source",
                    Some(serde_json::json!({ "kind": "same_session_replace" })),
                ));
            }
            let loaded = load_app_server_resume_session(
                SessionResumeInput {
                    target: target.clone(),
                },
                &state,
            )
            .await?;
            let destination_id = loaded.session_id.clone();
            if let Some(watermark) = loaded.session.session_seq_watermark {
                restore_session_seq_from_watermark(
                    &app_server,
                    &state,
                    destination_id.clone(),
                    watermark,
                );
            }
            if let Some(handle) = app_server.registry().get(&destination_id) {
                let runtime = handle.clone().into_session();
                if !runtime
                    .callback_requirements()
                    .is_satisfied_by(&connection_profile)
                {
                    return Err(SessionOperationError::invalid_request(
                        "connection profile does not satisfy the live destination callback requirements",
                        Some(serde_json::json!({
                            "kind": "connection_profile_mismatch",
                            "session_id": destination_id,
                        })),
                    ));
                }
                (destination_id, handle, false)
            } else {
                let cwd = loaded.session.working_dir.clone();
                let prior_messages = loaded.conversation.messages.clone();
                let make_factory = || {
                    let state = Arc::clone(&state);
                    let replacement = replacement.clone();
                    let profile = Arc::clone(&connection_profile);
                    let session_id = destination_id.clone();
                    let cwd = cwd.clone();
                    let prior_messages = prior_messages.clone();
                    let app_server = Arc::clone(&app_server);
                    async move {
                        let runtime = build_connection_runtime_for_resume(
                            replacement,
                            state,
                            profile,
                            session_id.clone(),
                            cwd,
                            prior_messages,
                            app_server,
                        )
                        .await
                        .map_err(|error| {
                            coco_app_server::RegistryError::load_failed(error.to_string())
                        })?;
                        Ok::<_, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(
                            runtime,
                        ))
                    }
                };
                let handle = load_local_app_server_session_with_retrying_factory_parts(
                    &app_server,
                    destination_id.clone(),
                    make_factory,
                    turn_drain_timeout,
                )
                .await?;
                (destination_id, handle, true)
            }
        }
    };

    let destination_runtime = destination_handle.into_session();
    if configure_profile {
        install_app_server_sandbox_reload_subscription(
            &destination_runtime,
            Arc::clone(&app_server),
        )
        .await;
        configure_connection_mcp_bridge(
            &connection_profile,
            &destination_runtime,
            Arc::clone(&app_server),
        )
        .await;
    }

    let commit = match app_server.commit_replace_to_live_for_surface(
        &source_session_id,
        &destination_id,
        &source_surface_id,
    ) {
        Ok(commit) => commit,
        Err(error) => {
            // Freshly loaded replacement destinations have no owner until the
            // atomic commit succeeds. Do not leak that orphan when validation
            // races with a disconnect or another lifecycle operation.
            if configure_profile {
                let _ = close_local_app_server_session_parts(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    destination_id.clone(),
                    turn_drain_timeout,
                )
                .await;
            }
            return Err(app_server_lifecycle_error_parts("commit replacement", error).into());
        }
    };
    app_server.route_lifecycle_effects(commit.lifecycle_effects);
    let close_server = Arc::clone(&app_server);
    let close_state = Arc::clone(&state);
    tokio::spawn(async move {
        close_app_server_session_state(&close_state, &source_session_id).await;
        close_local_session_handle_with_reason(
            commit.old_handle,
            coco_hooks::orchestration::ExitReason::Other,
            turn_drain_timeout,
        )
        .await;
        if let Ok(archive) = close_server.complete_close_and_archive_surfaces(&source_session_id) {
            close_server.route_lifecycle_effects(archive.lifecycle_effects);
        }
    });

    Ok(SessionReplaceResult {
        session_id: destination_id,
        surface_id: source_surface_id,
    })
}
