use std::{future::Future, sync::Arc, time::Duration};

use coco_app_server::{AppReplaceStart, AppServer, ConnectionKey};
use coco_types::{SessionAccess, SessionReplaceResult};

use crate::app_server_host::connection_runtime_binding::{
    build_connection_runtime_for_clear, build_connection_runtime_for_resume,
    build_connection_runtime_for_start, configure_connection_mcp_bridge,
    install_app_server_sandbox_reload_subscription, install_connection_runtime_callbacks,
    register_connection_callback_owners,
};
use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};
use crate::app_session::AppSessionHandle;
use crate::session_resume::SessionResumeInput;

use super::session_close::{
    close_app_server_session_state, close_local_session_handle_with_reason,
};
use super::session_errors::app_server_lifecycle_error_parts;
use super::session_errors::local_lifecycle_error_parts;
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
        .validate_session_target(connection, &input.source, SessionAccess::Full)
        .map_err(|error| app_server_lifecycle_error_parts("validate replacement source", error))?;
    let source_session_id = input.source.session_id.clone();

    let (destination_id, destination_handle, needs_live_repoint, start_source) = match input
        .destination
    {
        SessionReplaceDestination::Fresh(start_input) => {
            let prepared =
                prepare_app_server_session_start(*start_input, &state, &connection_profile).await?;
            let destination_id = prepared.session_id.clone();
            let factory = {
                let replacement = replacement.clone();
                let profile = Arc::clone(&connection_profile);
                let app_server = Arc::clone(&app_server);
                async move {
                    let runtime = build_connection_runtime_for_start(
                        replacement,
                        Arc::clone(&profile),
                        prepared,
                        Arc::clone(&app_server),
                    )
                    .await
                    .map_err(|error| {
                        coco_app_server::RegistryError::load_failed(error.to_string())
                    })?;
                    install_app_server_sandbox_reload_subscription(
                        &runtime,
                        Arc::clone(&app_server),
                    )
                    .await;
                    Ok::<_, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(runtime))
                }
            };
            let handle = replace_app_server_session_with_factory(
                ReplacementReservation {
                    app_server: Arc::clone(&app_server),
                    state: Arc::clone(&state),
                    source_session_id: source_session_id.clone(),
                    destination_id: destination_id.clone(),
                    connection,
                    close_reason: coco_hooks::orchestration::ExitReason::Other,
                    turn_drain_timeout,
                },
                factory,
            )
            .await?;
            (
                destination_id,
                handle,
                false,
                Some(coco_hooks::orchestration::SessionStartSource::Startup),
            )
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
                    plan_mode_instructions: None,
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
                (destination_id, handle, true, None)
            } else {
                let cwd = loaded.session.working_dir.clone();
                let prior_messages = loaded.conversation.messages.clone();
                let persisted_mcp_tool_exposure = loaded.conversation.mcp_tool_exposure;
                let factory = {
                    let replacement = replacement.clone();
                    let profile = Arc::clone(&connection_profile);
                    let session_id = destination_id.clone();
                    let cwd = cwd.clone();
                    let prior_messages = prior_messages.clone();
                    let app_server = Arc::clone(&app_server);
                    async move {
                        let runtime = build_connection_runtime_for_resume(
                            replacement,
                            Arc::clone(&profile),
                            session_id.clone(),
                            cwd,
                            prior_messages,
                            // Replace-to-resume carries only a target; plan-mode
                            // policy is re-supplied via `session/resume`.
                            None,
                            persisted_mcp_tool_exposure,
                            Arc::clone(&app_server),
                        )
                        .await
                        .map_err(|error| {
                            coco_app_server::RegistryError::load_failed(error.to_string())
                        })?;
                        install_app_server_sandbox_reload_subscription(
                            &runtime,
                            Arc::clone(&app_server),
                        )
                        .await;
                        Ok::<_, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(
                            runtime,
                        ))
                    }
                };
                let handle = replace_app_server_session_with_factory(
                    ReplacementReservation {
                        app_server: Arc::clone(&app_server),
                        state: Arc::clone(&state),
                        source_session_id: source_session_id.clone(),
                        destination_id: destination_id.clone(),
                        connection,
                        close_reason: coco_hooks::orchestration::ExitReason::Other,
                        turn_drain_timeout,
                    },
                    factory,
                )
                .await?;
                (
                    destination_id,
                    handle,
                    false,
                    Some(coco_hooks::orchestration::SessionStartSource::Resume),
                )
            }
        }
        SessionReplaceDestination::Clear => {
            let source_handle = app_server
                .registry()
                .get(&source_session_id)
                .ok_or_else(|| {
                    SessionOperationError::invalid_request(
                        "session/replace clear source is not live",
                        Some(serde_json::json!({
                            "kind": "source_not_live",
                            "session_id": source_session_id.clone(),
                        })),
                    )
                })?;
            let source_runtime = source_handle.into_session();
            let snapshot = source_runtime.clear_replacement_snapshot().await;
            let destination_id = coco_types::SessionId::generate();
            let factory = {
                let replacement = replacement.clone();
                let profile = Arc::clone(&connection_profile);
                let session_id = destination_id.clone();
                let app_server = Arc::clone(&app_server);
                async move {
                    let runtime = build_connection_runtime_for_clear(
                        replacement,
                        profile,
                        session_id,
                        snapshot,
                        app_server,
                    )
                    .await
                    .map_err(|error| {
                        coco_app_server::RegistryError::load_failed(error.to_string())
                    })?;
                    Ok::<_, coco_app_server::RegistryError>(AppSessionHandle::from_runtime(runtime))
                }
            };
            let handle = replace_app_server_session_with_factory(
                ReplacementReservation {
                    app_server: Arc::clone(&app_server),
                    state: Arc::clone(&state),
                    source_session_id: source_session_id.clone(),
                    destination_id: destination_id.clone(),
                    connection,
                    close_reason: coco_hooks::orchestration::ExitReason::Clear,
                    turn_drain_timeout,
                },
                factory,
            )
            .await?;
            (
                destination_id,
                handle,
                false,
                Some(coco_hooks::orchestration::SessionStartSource::Clear),
            )
        }
    };

    let destination_runtime = destination_handle.into_session();

    if needs_live_repoint {
        // Run the whole commit + source-close through the AppServer owner-task
        // wrapper so the source close cascade is `OwnerGuard`-protected and
        // shutdown-tracked. A bare `tokio::spawn` here would wedge the source in
        // `Closing` forever on a cascade panic (and escape shutdown joining).
        let close_state = Arc::clone(&state);
        let close_source_id = source_session_id.clone();
        let close_old = move |old_handle: AppSessionHandle| {
            let close_state = Arc::clone(&close_state);
            let close_source_id = close_source_id.clone();
            async move {
                close_app_server_session_state(&close_state, &close_source_id).await;
                close_local_session_handle_with_reason(
                    old_handle,
                    coco_hooks::orchestration::ExitReason::Other,
                    turn_drain_timeout,
                )
                .await
            }
        };
        app_server
            .spawn_replace_to_live(
                source_session_id.clone(),
                destination_id.clone(),
                connection,
                close_old,
            )
            .map_err(|error| app_server_lifecycle_error_parts("commit replacement", error))?;
        install_connection_runtime_callbacks(
            &connection_profile,
            &destination_runtime,
            Arc::clone(&app_server),
            connection,
        )
        .map_err(callback_owner_registration_error)?;
    } else {
        register_connection_callback_owners(
            &connection_profile,
            &destination_runtime,
            &app_server,
            connection,
        )
        .map_err(callback_owner_registration_error)?;
    }
    if let Some(source) = start_source {
        destination_runtime.fire_session_start_hooks(source).await;
    }
    configure_connection_mcp_bridge(
        &connection_profile,
        &destination_runtime,
        Arc::clone(&app_server),
        connection,
    )
    .await;

    Ok(SessionReplaceResult {
        session_id: destination_id,
    })
}

fn callback_owner_registration_error(
    error: coco_app_server::SessionAccessError,
) -> SessionOperationError {
    SessionOperationError::internal(
        format!("register session/replace callback owners: {error}"),
        Some(serde_json::json!({ "kind": "callback_owner_registration_failed" })),
    )
}

struct ReplacementReservation {
    app_server: Arc<AppServer<AppSessionHandle>>,
    state: Arc<AppServerHostState>,
    source_session_id: coco_types::SessionId,
    destination_id: coco_types::SessionId,
    connection: ConnectionKey,
    close_reason: coco_hooks::orchestration::ExitReason,
    turn_drain_timeout: Duration,
}

async fn replace_app_server_session_with_factory<F>(
    reservation: ReplacementReservation,
    factory: F,
) -> Result<AppSessionHandle, SessionOperationError>
where
    F: Future<Output = Result<AppSessionHandle, coco_app_server::RegistryError>> + Send + 'static,
{
    let close_state = Arc::clone(&reservation.state);
    let close_reason = reservation.close_reason;
    let turn_drain_timeout = reservation.turn_drain_timeout;
    let mut completion = match reservation
        .app_server
        .spawn_replace(
            reservation.source_session_id,
            reservation.destination_id,
            reservation.connection,
            factory,
            move |handle| {
                let close_state = Arc::clone(&close_state);
                async move {
                    let source_session_id = handle.session_id().clone();
                    close_app_server_session_state(&close_state, &source_session_id).await;
                    let result = close_local_session_handle_with_reason(
                        handle,
                        close_reason,
                        turn_drain_timeout,
                    )
                    .await;
                    if let Err(error) = &result {
                        // Post-commit source close failed after the replacement was
                        // committed; the destination is live, so this is never a
                        // clean success (CS-3b). Surface it as a structured error
                        // rather than dropping it.
                        tracing::error!(
                            %source_session_id,
                            %error,
                            kind = "committed_close_failed",
                            "session/replace committed the destination but the source close failed",
                        );
                    }
                    result
                }
            },
        )
        .map_err(|error| app_server_lifecycle_error_parts("reserve replacement", error))?
    {
        AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error_parts("replace session", error).into())
}
