use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use coco_app_server::{
    AppCloseStart, AppLoadStart, AppServer, AttachSurfaceOptions, ConnectionKey,
    JsonRpcDispatchError, SubscribeReplay, SurfaceRole,
};
use coco_types::{ClientRequest, CoreEvent, SessionEnvelope, SessionId, SurfaceId};
use tracing::{debug, warn};

use super::{
    app_server_bridge::{APP_SERVER_TURN_DRAIN_TIMEOUT, LocalAppSessionHandle},
    handlers::{
        HandlerResult, PriorSessionCleanup, ReplacementSessionState, RuntimeReplacementContext,
        SdkServerState, SessionHandoffState, SessionMetadata, session,
    },
};

#[derive(Debug, Clone)]
pub(super) enum LocalLifecycleRequest {
    Start {
        connection: ConnectionKey,
    },
    Resume {
        connection: ConnectionKey,
    },
    Archive {
        connection: ConnectionKey,
        target: coco_types::ArchiveTarget,
    },
}

impl LocalLifecycleRequest {
    pub(super) fn from_client_request(
        connection: ConnectionKey,
        request: &ClientRequest,
    ) -> Option<Self> {
        // no process-wide `live_before` snapshot. `session/start`
        // creates a new slot and `session/resume` replaces only the requesting
        // connection's own current session — neither closes other sessions.
        match request {
            ClientRequest::SessionStart(_) => Some(Self::Start { connection }),
            ClientRequest::SessionResume(_) => Some(Self::Resume { connection }),
            ClientRequest::SessionArchive(params) => Some(Self::Archive {
                connection,
                target: params.target.clone(),
            }),
            _ => None,
        }
    }
}

pub(super) async fn start_sdk_session_with_scoped_state(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionStartParams,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let prepared = session::prepare_session_start(params, &state, &connection_profile)
        .await
        .map_err(handler_result_to_dispatch_error)?;

    let mut result = serde_json::to_value(coco_types::SessionStartResult {
        session_id: prepared.session_id.clone(),
        surface_id: None,
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/start result encode failed: {error}"),
        data: None,
    })?;
    if let Some(surface_id) = apply_local_lifecycle_request(
        app_server,
        Arc::clone(&state),
        LocalLifecycleRequest::Start { connection },
        &result,
        turn_drain_timeout,
    )
    .await?
    {
        inject_surface_id(&mut result, surface_id)?;
    }
    session::install_scoped_started_session_state(&state, &prepared, None).await;
    Ok(result)
}

pub(super) async fn resume_sdk_session_with_scoped_state(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionResumeParams,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let mut result = encode_session_resume_result(&loaded.session, None)?;
    if let Some(surface_id) = apply_local_lifecycle_request(
        app_server,
        Arc::clone(&state),
        LocalLifecycleRequest::Resume { connection },
        &result,
        turn_drain_timeout,
    )
    .await?
    {
        inject_surface_id(&mut result, surface_id)?;
    }
    session::install_scoped_resumed_session_state(
        &state,
        &loaded.session,
        loaded.session_id.clone(),
        &loaded.conversation.messages,
        &connection_profile,
    )
    .await;
    Ok(result)
}

pub(super) async fn start_sdk_session_with_runtime_replacement(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionStartParams,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let prepared = session::prepare_session_start(params, &state, &connection_profile)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let started_session_id = prepared.session_id.clone();
    let startup_session_id = replacement.startup_session_id.clone();

    let factory = {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let prepared = prepared.clone();
        let connection_profile = Arc::clone(&connection_profile);
        let app_server = Arc::clone(&app_server);
        async move {
            let session_id = prepared.session_id.clone();
            let runtime = build_sdk_runtime_for_start(
                replacement,
                state,
                connection_profile,
                prepared,
                app_server,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(session_id, runtime),
            )
        }
    };

    // The bootstrap runtime is an implementation placeholder, not a client
    // session. Replace that one explicit surfaceless slot on the first start;
    // after it is gone, every later start creates a new slot and never closes
    // another user's session.
    let handle =
        if registered_detached_session(&app_server, &startup_session_id, &started_session_id) {
            replace_detached_local_app_server_session_with_factory(
                Arc::clone(&app_server),
                Arc::clone(&state),
                startup_session_id,
                started_session_id.clone(),
                factory,
                turn_drain_timeout,
            )
            .await?
        } else {
            load_local_app_server_session_with_factory(
                &app_server,
                started_session_id.clone(),
                factory,
            )
            .await?
        };
    let runtime = handle.require_runtime("started")?;

    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone(), Arc::clone(&app_server))
        .await;
    session::install_scoped_started_session_state(
        &state,
        &prepared,
        Some(Arc::clone(runtime.app_state())),
    )
    .await;

    let surface_id =
        attach_local_app_server_surface(&app_server, connection, started_session_id.clone())?;
    configure_sdk_mcp(&connection_profile, &runtime, Arc::clone(&app_server)).await;
    serde_json::to_value(coco_types::SessionStartResult {
        session_id: started_session_id,
        surface_id: Some(surface_id),
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/start result encode failed: {error}"),
        data: None,
    })
}

pub(super) async fn resume_sdk_session_with_runtime_replacement(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionResumeParams,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let resumed_session_id = loaded.session_id.clone();
    let resumed_cwd = loaded.session.working_dir.clone();
    let prior_messages = loaded.conversation.messages.clone();

    if let Some(handle) = app_server.registry().get(&resumed_session_id) {
        let runtime = handle.require_runtime("resumed")?;
        if !runtime
            .callback_requirements()
            .is_satisfied_by(&connection_profile)
        {
            return Err(JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_REQUEST,
                message:
                    "connection profile does not satisfy the live session callback requirements"
                        .to_string(),
                data: Some(serde_json::json!({
                    "kind": "connection_profile_mismatch",
                    "session_id": resumed_session_id,
                })),
            });
        }
        let surface_id =
            attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
        return encode_session_resume_result(&loaded.session, Some(surface_id));
    }

    let make_factory = || {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let session_id = resumed_session_id.clone();
        let cwd = resumed_cwd.clone();
        let prior_messages = prior_messages.clone();
        let connection_profile = Arc::clone(&connection_profile);
        let app_server = Arc::clone(&app_server);
        async move {
            let runtime = build_sdk_runtime_for_resume(
                replacement,
                state,
                connection_profile,
                session_id.clone(),
                cwd,
                prior_messages,
                app_server,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(session_id, runtime),
            )
        }
    };

    let handle = if registered_detached_session(
        &app_server,
        &replacement.startup_session_id,
        &resumed_session_id,
    ) {
        replace_detached_local_app_server_session_with_factory(
            Arc::clone(&app_server),
            Arc::clone(&state),
            replacement.startup_session_id.clone(),
            resumed_session_id.clone(),
            make_factory(),
            turn_drain_timeout,
        )
        .await?
    } else {
        load_local_app_server_session_with_retrying_factory(
            &app_server,
            resumed_session_id.clone(),
            make_factory,
            turn_drain_timeout,
        )
        .await?
    };
    let runtime = handle.require_runtime("resumed")?;
    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone(), Arc::clone(&app_server))
        .await;
    install_runtime_backed_resumed_sdk_session_state(
        &state,
        &loaded.session,
        resumed_session_id.clone(),
        &runtime,
        &prior_messages,
        &connection_profile,
    )
    .await;

    let surface_id = attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
    configure_sdk_mcp(&connection_profile, &runtime, Arc::clone(&app_server)).await;
    encode_session_resume_result(&loaded.session, Some(surface_id))
}

pub(super) async fn replace_sdk_session_with_runtime(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionReplaceParams,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    app_server
        .validate_interactive_target(connection, &params.source)
        .map_err(|error| app_server_lifecycle_error("validate replacement source", error))?;
    let source_session_id = params.source.session_id.clone();
    let source_surface_id = params.source.surface_id.clone();

    let (destination_id, destination_handle, configure_profile) = match params.destination {
        coco_types::SessionReplacement::Fresh(start_params) => {
            let prepared =
                session::prepare_session_start(start_params, &state, &connection_profile)
                    .await
                    .map_err(handler_result_to_dispatch_error)?;
            let destination_id = prepared.session_id.clone();
            let factory = {
                let factory_session_id = destination_id.clone();
                let state = Arc::clone(&state);
                let replacement = replacement.clone();
                let profile = Arc::clone(&connection_profile);
                let app_server = Arc::clone(&app_server);
                async move {
                    let runtime = build_sdk_runtime_for_start(
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
                    Ok::<_, coco_app_server::RegistryError>(LocalAppSessionHandle::from_runtime(
                        factory_session_id,
                        runtime,
                    ))
                }
            };
            let handle = load_local_app_server_session_with_factory(
                &app_server,
                destination_id.clone(),
                factory,
            )
            .await?;
            (destination_id, handle, true)
        }
        coco_types::SessionReplacement::Resume(target) => {
            if target.session_id == source_session_id {
                return Err(JsonRpcDispatchError {
                    code: coco_types::error_codes::INVALID_PARAMS,
                    message: "session/replace destination must differ from its source".to_string(),
                    data: Some(serde_json::json!({ "kind": "same_session_replace" })),
                });
            }
            let loaded = session::load_resume_session(
                coco_types::SessionResumeParams {
                    target: target.clone(),
                },
                &state,
            )
            .await
            .map_err(handler_result_to_dispatch_error)?;
            let destination_id = loaded.session_id.clone();
            if let Some(handle) = app_server.registry().get(&destination_id) {
                let runtime = handle.clone().require_runtime("replacement destination")?;
                if !runtime
                    .callback_requirements()
                    .is_satisfied_by(&connection_profile)
                {
                    return Err(JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: "connection profile does not satisfy the live destination callback requirements".to_string(),
                        data: Some(serde_json::json!({
                            "kind": "connection_profile_mismatch",
                            "session_id": destination_id,
                        })),
                    });
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
                        let runtime = build_sdk_runtime_for_resume(
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
                        Ok::<_, coco_app_server::RegistryError>(
                            LocalAppSessionHandle::from_runtime(session_id, runtime),
                        )
                    }
                };
                let handle = load_local_app_server_session_with_retrying_factory(
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

    let destination_runtime = destination_handle.require_runtime("replacement destination")?;
    if configure_profile {
        install_sdk_runtime_reload_subscription(&destination_runtime, Arc::clone(&app_server))
            .await;
        configure_sdk_mcp(
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
                let _ = close_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    destination_id.clone(),
                    turn_drain_timeout,
                )
                .await;
            }
            return Err(app_server_lifecycle_error("commit replacement", error));
        }
    };
    app_server.route_lifecycle_effects(commit.lifecycle_effects);
    let close_server = Arc::clone(&app_server);
    let close_state = Arc::clone(&state);
    tokio::spawn(async move {
        close_sdk_session_state_for_app_server(
            &close_state,
            &source_session_id,
            turn_drain_timeout,
        )
        .await;
        close_local_session_handle_with_reason(
            commit.old_handle,
            coco_hooks::orchestration::ExitReason::Other,
        )
        .await;
        if let Ok(archive) = close_server.complete_close_and_archive_surfaces(&source_session_id) {
            close_server.route_lifecycle_effects(archive.lifecycle_effects);
        }
    });

    serde_json::to_value(coco_types::SessionReplaceResult {
        session_id: destination_id,
        surface_id: source_surface_id,
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/replace result encode failed: {error}"),
        data: None,
    })
}

async fn configure_sdk_mcp(
    profile: &coco_types::ConnectionProfile,
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) {
    let Some(server_names) = profile.initialize().sdk_mcp_servers.clone() else {
        return;
    };
    if server_names.is_empty() {
        return;
    }
    if let Err(error) =
        crate::sdk_server::sdk_mcp::register_and_connect(session.clone(), app_server, server_names)
            .await
    {
        warn!(session_id = %session.session_id(), %error, "SDK MCP registration failed");
    }
}

pub(super) async fn build_sdk_runtime_for_start(
    replacement: RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    prepared: session::PreparedStartSession,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            prepared.session_id.clone(),
            session_build_cwd_from_str(&replacement.cwd, &prepared.cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(
        &replacement,
        state,
        connection_profile,
        &session,
        app_server,
    )
    .await?;
    session.fire_session_start_hooks("startup").await;
    Ok(session)
}

pub(super) fn session_build_cwd_from_str(base: &std::path::Path, cwd: &str) -> std::path::PathBuf {
    session_build_cwd(base, std::path::Path::new(cwd))
}

pub(super) fn session_build_cwd(
    base: &std::path::Path,
    cwd: &std::path::Path,
) -> std::path::PathBuf {
    if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        base.join(cwd)
    }
}

pub(super) async fn build_sdk_runtime_for_resume(
    replacement: RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    session_id: SessionId,
    cwd: std::path::PathBuf,
    prior_messages: Vec<coco_messages::Message>,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            session_id.clone(),
            session_build_cwd(&replacement.cwd, &cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(
        &replacement,
        state,
        connection_profile,
        &session,
        app_server,
    )
    .await?;
    session::hydrate_runtime_for_resume_messages(&session, &session_id, &prior_messages).await;
    session.fire_session_start_hooks("resume").await;
    Ok(session)
}

pub(super) async fn setup_sdk_replacement_runtime(
    replacement: &RuntimeReplacementContext,
    _state: Arc<SdkServerState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) -> anyhow::Result<()> {
    let runtime = session;
    runtime.install_callback_requirements(connection_profile.callback_requirements());
    let session_cwd = runtime.original_cwd().clone();
    if replacement.requires_structured_output {
        runtime
            .update_engine_config(|cfg| cfg.requires_structured_output = true)
            .await;
    }
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&app_server), session);
    if let Some(hooks) = &connection_profile.initialize().hooks {
        crate::sdk_server::sdk_hooks::register_initialize_hooks(session, hooks);
    }
    let sdk_agents = connection_profile
        .initialize()
        .agents
        .as_ref()
        .map(crate::sdk_server::cli_bootstrap::parse_sdk_agent_definitions)
        .map(|(accepted, _)| accepted)
        .unwrap_or_default();
    if !sdk_agents.is_empty() {
        session.set_sdk_supplied_agents(sdk_agents).await;
    }

    let lsp_handle = crate::session_bootstrap::build_lsp_handle_if_enabled(
        Arc::clone(&replacement.process_runtime),
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    crate::session_bootstrap::install_session_late_binds(
        session.clone(),
        &session_cwd,
        None,
        lsp_handle,
        None,
    )
    .await?;
    crate::session_bootstrap::bootstrap_session_mcp(
        session,
        &session_cwd,
        None,
        /*await_connect*/ false,
    )
    .await;
    crate::leader_inbox_poller::install_leader(session.clone(), None).await;
    Ok(())
}

pub async fn install_sdk_session_runtime_state(
    state: Arc<SdkServerState>,
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) {
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&app_server), &session);
    let runtime = &session;
    let session_manager = Arc::clone(runtime.session_manager());
    install_sdk_runtime_reload_subscription(&session, app_server).await;
    let _ = session;
    state.install_session_manager(session_manager).await;
}

pub(super) async fn install_sdk_runtime_reload_subscription(
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
) {
    let runtime = session;
    let Some(sandbox_state) = runtime.sandbox_state() else {
        return;
    };
    sandbox_state.set_approval_bridge(Arc::new(crate::sdk_server::SdkSandboxApprovalBridge::new(
        app_server,
        session.clone(),
    )));

    let Some(publisher) = runtime.runtime_publisher() else {
        return;
    };
    session
        .install_reload_supervisor(crate::sandbox_reload::spawn_sandbox_reload(
            sandbox_state,
            &publisher,
            runtime.original_cwd().clone(),
        ))
        .await;
}

pub(super) async fn install_runtime_backed_resumed_sdk_session_state(
    state: &SdkServerState,
    session: &coco_session::Session,
    session_id: SessionId,
    runtime_handle: &crate::session_runtime::SessionHandle,
    prior_messages: &[coco_messages::Message],
    connection_profile: &coco_types::ConnectionProfile,
) {
    let plan_mode_instructions = connection_profile
        .initialize()
        .plan_mode_instructions
        .clone();
    let runtime = runtime_handle;
    let history = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();

    state.install_scoped_replacement_session_state(ReplacementSessionState {
        session_id,
        metadata: SessionMetadata {
            cwd: session.working_dir.to_string_lossy().into_owned(),
            model: session.model.clone(),
        },
        handoff: SessionHandoffState {
            history: Arc::new(tokio::sync::Mutex::new(history)),
            app_state: Arc::clone(runtime.app_state()),
        },
        plan_mode_instructions,
        prior_cleanup: PriorSessionCleanup::ActiveTurnAndHandoff,
        reset_accounting: false,
    });
}

pub(super) async fn load_local_app_server_session_with_factory<F>(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
    factory: F,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let mut completion = match app_server
        .spawn_load(session_id.clone(), factory)
        .map_err(|error| local_lifecycle_error("load session", error))?
    {
        AppLoadStart::Started { completion } | AppLoadStart::Loading(completion) => completion,
        AppLoadStart::Live(handle) => return Ok(handle),
        AppLoadStart::Closing(_) => {
            return Err(JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session {session_id} is closing"),
                data: None,
            });
        }
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("load session", error))
}

pub(super) async fn load_local_app_server_session_with_retrying_factory<Make, F>(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
    make_factory: Make,
    close_wait_timeout: Duration,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError>
where
    Make: Fn() -> F,
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    loop {
        match app_server
            .spawn_load(session_id.clone(), make_factory())
            .map_err(|error| local_lifecycle_error("load session", error))?
        {
            AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
                return completion
                    .wait()
                    .await
                    .map_err(|error| local_lifecycle_error("load session", error));
            }
            AppLoadStart::Live(handle) => return Ok(handle),
            AppLoadStart::Closing(mut completion) => {
                tokio::time::timeout(close_wait_timeout, completion.wait())
                    .await
                    .map_err(|_| JsonRpcDispatchError {
                        code: coco_types::error_codes::INTERNAL_ERROR,
                        message: format!(
                            "timed out waiting for closing session {session_id} before resume"
                        ),
                        data: Some(serde_json::json!({
                            "kind": "session_close_timeout",
                            "session_id": session_id,
                        })),
                    })?
                    .map_err(|error| local_lifecycle_error("wait for closing session", error))?;
            }
        }
    }
}

pub async fn load_local_app_server_session_runtime(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError> {
    let registry_session_id = session_id.clone();
    let build_session_id = session_id.clone();
    load_local_app_server_session_with_factory(app_server, session_id, async move {
        let runtime = runtime_factory
            .build_with_session_id(build_session_id)
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
        Ok(LocalAppSessionHandle::from_runtime(
            registry_session_id,
            runtime,
        ))
    })
    .await
}

pub async fn load_local_app_server_session_runtime_with_cwd(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    cwd: PathBuf,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError> {
    let registry_session_id = session_id.clone();
    let build_session_id = session_id.clone();
    load_local_app_server_session_with_factory(app_server, session_id, async move {
        let runtime = runtime_factory
            .build_with_session_id_and_cwd(build_session_id, cwd)
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
        Ok(LocalAppSessionHandle::from_runtime(
            registry_session_id,
            runtime,
        ))
    })
    .await
}

pub(super) fn encode_session_resume_result(
    session: &coco_session::Session,
    surface_id: Option<SurfaceId>,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let summary = session::session_to_summary(session).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/resume returned invalid session id: {error}"),
        data: None,
    })?;
    serde_json::to_value(coco_types::SessionResumeResult {
        session: summary,
        surface_id,
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/resume result encode failed: {error}"),
        data: None,
    })
}

pub(super) fn handler_result_to_dispatch_error(error: HandlerResult) -> JsonRpcDispatchError {
    match error {
        HandlerResult::Err {
            code,
            message,
            data,
        } => JsonRpcDispatchError {
            code,
            message,
            data,
        },
        HandlerResult::NotImplemented(method) => JsonRpcDispatchError::method_not_found(method),
        HandlerResult::Ok(_) => JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "session/resume loader returned an unexpected success result".to_string(),
            data: None,
        },
    }
}

pub(super) async fn apply_local_lifecycle_request(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    request: LocalLifecycleRequest,
    result: &serde_json::Value,
    turn_drain_timeout: Duration,
) -> Result<Option<SurfaceId>, JsonRpcDispatchError> {
    match request {
        LocalLifecycleRequest::Start { connection } => {
            let started: coco_types::SessionStartResult = serde_json::from_value(result.clone())
                .map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/start decode failed: {error}"),
                    data: None,
                })?;
            let session_id = started.session_id;
            let new_handle = LocalAppSessionHandle::snapshot(session_id.clone());
            if let Some(placeholder) = single_session_detached_placeholder(&app_server, &session_id)
            {
                replace_detached_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    placeholder,
                    new_handle,
                    turn_drain_timeout,
                )
                .await?;
            } else {
                // Multi-session hosts create a new slot and leave other user
                // sessions untouched. A single-session local bridge may reuse
                // its sole slot only after it has lost interactive ownership.
                register_local_app_server_session(&app_server, new_handle).await?;
            }
            let surface_id = attach_local_app_server_surface(&app_server, connection, session_id)?;
            Ok(Some(surface_id))
        }
        LocalLifecycleRequest::Resume { connection } => {
            let resumed: coco_types::SessionResumeResult =
                serde_json::from_value(result.clone()).map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/resume decode failed: {error}"),
                    data: None,
                })?;
            let resumed_session_id = resumed.session.session_id;
            let new_handle = LocalAppSessionHandle::snapshot(resumed_session_id.clone());
            if let Some(placeholder) =
                single_session_detached_placeholder(&app_server, &resumed_session_id)
            {
                replace_detached_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    placeholder,
                    new_handle,
                    turn_drain_timeout,
                )
                .await?;
            } else {
                register_local_app_server_session(&app_server, new_handle).await?;
            }
            let surface_id =
                attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
            Ok(Some(surface_id))
        }
        LocalLifecycleRequest::Archive { connection, target } => {
            let session_id = target.session_id().clone();
            match &target {
                coco_types::ArchiveTarget::Interactive(target) => {
                    app_server
                        .validate_interactive_target(connection, target)
                        .map_err(|error| {
                            app_server_lifecycle_error("validate archive target", error)
                        })?;
                }
                coco_types::ArchiveTarget::Orphaned(target) => {
                    close_orphan_local_app_server_session(
                        app_server,
                        state,
                        target.session_id.clone(),
                        turn_drain_timeout,
                    )
                    .await?;
                    return Ok(None);
                }
            }
            close_local_app_server_session(app_server, state, session_id, turn_drain_timeout)
                .await?;
            Ok(None)
        }
    }
}

pub(super) fn single_session_detached_placeholder(
    app_server: &AppServer<LocalAppSessionHandle>,
    target_session_id: &SessionId,
) -> Option<SessionId> {
    if app_server.registry().max_sessions() != 1 {
        return None;
    }
    let live = app_server.list_live_sessions();
    match live.as_slice() {
        [summary]
            if summary.session_id != *target_session_id
                && app_server
                    .routing()
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .interactive_owner(&summary.session_id)
                    .is_none() =>
        {
            Some(summary.session_id.clone())
        }
        [] | [_] | [_, ..] => None,
    }
}

pub(super) fn registered_detached_session(
    app_server: &AppServer<LocalAppSessionHandle>,
    candidate: &SessionId,
    target_session_id: &SessionId,
) -> bool {
    candidate != target_session_id
        && app_server.list_live_sessions().iter().any(|summary| {
            summary.session_id == *candidate
                && app_server
                    .routing()
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .interactive_owner(&summary.session_id)
                    .is_none()
        })
}

pub(super) fn inject_surface_id(
    result: &mut serde_json::Value,
    surface_id: SurfaceId,
) -> Result<(), JsonRpcDispatchError> {
    let Some(object) = result.as_object_mut() else {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "local AppServer lifecycle result was not an object".to_string(),
            data: None,
        });
    };
    object.insert(
        "surface_id".to_string(),
        serde_json::to_value(surface_id).map_err(|error| JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("local AppServer surface id encode failed: {error}"),
            data: None,
        })?,
    );
    Ok(())
}

pub(super) fn attach_local_app_server_surface(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    connection: ConnectionKey,
    session_id: SessionId,
) -> Result<SurfaceId, JsonRpcDispatchError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..Default::default()
    };
    app_server
        .attach_surface_with_options(connection, surface_id.clone(), session_id, options)
        .map_err(|error| attach_lifecycle_error("attach session surface", error))?;
    Ok(surface_id)
}

pub(super) async fn subscribe_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    connection: ConnectionKey,
    params: coco_types::SessionSubscribeParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Passive,
        ..Default::default()
    };
    match app_server
        .subscribe_surface_with_options(
            connection,
            surface_id.clone(),
            params.target.session_id.clone(),
            params.after_seq,
            options,
        )
        .map_err(|error| local_lifecycle_error("subscribe session", error))?
    {
        SubscribeReplay::Replayed(replayed) => {
            let replayed = replayed
                .into_iter()
                .map(encode_session_subscribe_envelope)
                .collect();
            serde_json::to_value(coco_types::SessionSubscribeResult {
                session_id: params.target.session_id,
                surface_id,
                replayed,
            })
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/subscribe encode failed: {error}"),
                data: None,
            })
        }
        SubscribeReplay::SnapshotRequired => Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/subscribe requires a fresh snapshot before passive attach"
                .to_string(),
            data: Some(serde_json::json!({ "kind": "snapshot_required" })),
        }),
    }
}

pub(super) fn encode_session_subscribe_envelope(
    envelope: SessionEnvelope,
) -> coco_types::SessionSubscribeEnvelope {
    let event = match envelope.event {
        CoreEvent::Protocol(notification) => serde_json::json!({
            "layer": "protocol",
            "payload": notification,
        }),
        CoreEvent::Stream(event) => serde_json::json!({
            "layer": "stream",
            "payload": event,
        }),
        CoreEvent::Tui(event) => serde_json::json!({
            "layer": "tui",
            "payload": event,
        }),
    };
    coco_types::SessionSubscribeEnvelope {
        session_id: envelope.session_id,
        agent_id: envelope.agent_id.map(coco_types::AgentId::into_inner),
        turn_id: envelope.turn_id,
        session_seq: envelope.session_seq,
        event,
    }
}

pub(super) async fn replace_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<Option<(LocalAppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    replace_local_app_server_session_with_factory_and_close_reason(
        app_server,
        state,
        old_session_id,
        new_session_id,
        factory,
        coco_hooks::orchestration::ExitReason::Other,
        turn_drain_timeout,
    )
    .await
}

pub(super) async fn replace_local_app_server_session_with_factory_and_close_reason<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    close_reason: coco_hooks::orchestration::ExitReason,
    turn_drain_timeout: Duration,
) -> Result<Option<(LocalAppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let Some(calling_surface) = local_replace_calling_surface(&app_server, &old_session_id) else {
        return Ok(None);
    };
    let calling_surface_id = calling_surface.clone();
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace(
            old_session_id,
            new_session_id,
            calling_surface,
            factory,
            move |handle| async move {
                close_sdk_session_state_for_app_server(
                    &close_state,
                    handle.session_id(),
                    turn_drain_timeout,
                )
                .await;
                close_local_session_handle_with_reason(handle, close_reason).await;
            },
        )
        .map_err(|error| app_server_lifecycle_error("replace session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map(|handle| Some((handle, calling_surface_id)))
        .map_err(|error| local_lifecycle_error("replace session", error))
}

pub(super) async fn replace_detached_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_handle: LocalAppSessionHandle,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    let new_session_id = new_handle.session_id().clone();
    replace_detached_local_app_server_session_with_factory(
        app_server,
        state,
        old_session_id,
        new_session_id,
        async { Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(new_handle) },
        turn_drain_timeout,
    )
    .await
    .map(|_| ())
}

pub(super) async fn replace_detached_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    turn_drain_timeout: Duration,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace_detached(
            old_session_id,
            new_session_id,
            factory,
            move |handle| async move {
                close_sdk_session_state_for_app_server(
                    &close_state,
                    handle.session_id(),
                    turn_drain_timeout,
                )
                .await;
                close_local_session_handle(handle).await;
            },
        )
        .map_err(|error| app_server_lifecycle_error("replace detached session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("replace detached session", error))
}

pub(super) fn local_replace_calling_surface(
    app_server: &AppServer<LocalAppSessionHandle>,
    session_id: &SessionId,
) -> Option<SurfaceId> {
    let routing = app_server
        .routing()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    routing.interactive_owner(session_id).cloned()
}

pub(super) async fn register_local_app_server_session(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    handle: LocalAppSessionHandle,
) -> Result<(), JsonRpcDispatchError> {
    let session_id = handle.session_id().clone();
    let handle_for_load = handle.clone();
    match app_server
        .spawn_load(session_id, async {
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(handle_for_load)
        })
        .map_err(|error| local_lifecycle_error("register session", error))?
    {
        AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
            completion
                .wait()
                .await
                .map(|_| ())
                .map_err(|error| local_lifecycle_error("register session", error))
        }
        AppLoadStart::Live(_) => {
            if handle.has_runtime() {
                let refresh_session_id = handle.session_id().clone();
                app_server
                    .registry()
                    .replace_live_handle(&refresh_session_id, handle)
                    .map_err(|error| local_lifecycle_error("refresh live session", error))?;
            }
            Ok(())
        }
        AppLoadStart::Closing(_) => Ok(()),
    }
}

pub(super) async fn close_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    if !app_server
        .list_live_sessions()
        .iter()
        .any(|summary| summary.session_id == session_id)
    {
        return Ok(());
    }
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_close(session_id, move |handle| async move {
            close_sdk_session_state_for_app_server(
                &close_state,
                handle.session_id(),
                turn_drain_timeout,
            )
            .await;
            close_local_session_handle(handle).await;
        })
        .map_err(|error| local_lifecycle_error("archive session", error))?
    {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("archive session", error))
}

async fn close_orphan_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    session_id: SessionId,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    let close_state = Arc::clone(&state);
    let mut completion = app_server
        .spawn_close_orphan(session_id, move |handle| async move {
            close_sdk_session_state_for_app_server(
                &close_state,
                handle.session_id(),
                turn_drain_timeout,
            )
            .await;
            close_local_session_handle(handle).await;
        })
        .map_err(|error| local_lifecycle_error("archive orphan session", error))?
        .completion();
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("archive orphan session", error))
}

pub async fn shutdown_local_app_server_sessions(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    turn_drain_timeout: Duration,
) -> Result<(), JsonRpcDispatchError> {
    let close_state = Arc::clone(&state);
    let shutdown = app_server.spawn_shutdown(move |handle| {
        let close_state = Arc::clone(&close_state);
        async move {
            close_sdk_session_state_for_app_server(
                &close_state,
                handle.session_id(),
                turn_drain_timeout,
            )
            .await;
            close_local_session_handle(handle).await;
        }
    });

    let mut first_error = shutdown
        .errors
        .into_iter()
        .next()
        .map(|(_, error)| local_lifecycle_error("shutdown sessions", error));
    for session in shutdown.sessions {
        let mut completion = session.completion;
        if let Err(error) = completion.wait().await
            && first_error.is_none()
        {
            first_error = Some(local_lifecycle_error("shutdown sessions", error));
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(super) async fn close_local_session_handle(handle: LocalAppSessionHandle) {
    close_local_session_handle_with_reason(handle, coco_hooks::orchestration::ExitReason::Other)
        .await;
}

pub(super) async fn close_local_session_handle_with_reason(
    handle: LocalAppSessionHandle,
    reason: coco_hooks::orchestration::ExitReason,
) {
    let has_runtime = handle.runtime().is_some();
    if let Some(runtime) = handle.runtime()
        && let Some(current_session_id) = runtime
            .close_if_current_session(handle.session_id(), reason, APP_SERVER_TURN_DRAIN_TIMEOUT)
            .await
    {
        debug!(
            target: "coco::app_server_local",
            registry_session_id = %handle.session_id(),
            current_session_id = %current_session_id,
            "skipping local AppServer close cascade for stale registry snapshot"
        );
        return;
    }
    debug!(
        target: "coco::app_server_local",
        session_id = %handle.session_id(),
        has_runtime,
        "local AppServer close cascade completed fused runtime boundary"
    );
}

pub(super) async fn close_sdk_session_state_for_app_server(
    state: &SdkServerState,
    session_id: &SessionId,
    turn_drain_timeout: Duration,
) {
    persist_session_seq_watermark_on_close(state, session_id).await;

    let active_turn = state.clear_scoped_session_state(session_id).await;

    if let Some(active_turn) = &active_turn {
        active_turn.cancel_token.cancel();
    }

    if let Some(active_turn) = active_turn {
        match tokio::time::timeout(turn_drain_timeout, active_turn.turn_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "local AppServer close: turn task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                timeout_secs = turn_drain_timeout.as_secs(),
                "local AppServer close: turn task did not drain before timeout"
            ),
        }
        match tokio::time::timeout(turn_drain_timeout, active_turn.forwarder_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "local AppServer close: forwarder task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                timeout_secs = turn_drain_timeout.as_secs(),
                "local AppServer close: forwarder task did not drain before timeout"
            ),
        }
    }
}

/// Persist the exact `session_seq` high-water mark before a session closes so a
/// later resume skips ahead from the true final value rather than a stale
/// interval watermark. Awaited (not best-effort) so a clean
/// shutdown always records an exact anchor.
pub(super) async fn persist_session_seq_watermark_on_close(
    state: &SdkServerState,
    session_id: &SessionId,
) {
    let Some(high_water) = state.session_seq_allocator().high_water(session_id) else {
        return;
    };
    let Some(manager) = state.session_manager_snapshot().await else {
        return;
    };
    let id = session_id.as_str().to_string();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(error) = manager.persist_session_seq_watermark(&id, high_water) {
            tracing::debug!(session_id = %id, %error, "failed to persist session_seq watermark at close");
        }
    })
    .await;
}

pub(super) fn local_lifecycle_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("local AppServer {operation} failed: {error}"),
        data: None,
    }
}

pub(super) fn app_server_lifecycle_error(
    operation: &'static str,
    error: coco_app_server::AppServerError,
) -> JsonRpcDispatchError {
    use coco_app_server::AppServerError;

    let (code, data) = match &error {
        AppServerError::Registry { source, .. } => {
            return registry_lifecycle_error(operation, source.clone());
        }
        AppServerError::CallingSurfaceNotAttached { surface_id, .. } => (
            coco_types::error_codes::INVALID_PARAMS,
            serde_json::json!({ "kind": "surface_not_attached", "surface_id": surface_id }),
        ),
        AppServerError::CallingSurfaceWrongSession {
            surface_id,
            expected_session_id,
            actual_session_id,
            ..
        } => (
            coco_types::error_codes::INVALID_PARAMS,
            serde_json::json!({
                "kind": "surface_wrong_session",
                "surface_id": surface_id,
                "expected_session_id": expected_session_id,
                "actual_session_id": actual_session_id,
            }),
        ),
        AppServerError::CallingSurfaceWrongConnection { surface_id, .. } => (
            coco_types::error_codes::PERMISSION_DENIED,
            serde_json::json!({ "kind": "surface_wrong_connection", "surface_id": surface_id }),
        ),
        AppServerError::CallingSurfaceNotInteractive { surface_id, .. } => (
            coco_types::error_codes::INVALID_PARAMS,
            serde_json::json!({ "kind": "surface_not_interactive", "surface_id": surface_id }),
        ),
        AppServerError::InteractiveOwnerConflict { session_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "interactive_owner_conflict", "session_id": session_id }),
        ),
        AppServerError::TargetSessionNotLive {
            session_id, state, ..
        } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "target_session_not_live", "session_id": session_id, "state": state }),
        ),
        AppServerError::ServerRequestNotFound { request_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "server_request_not_found", "request_id": request_id }),
        ),
        AppServerError::ServerRequestWrongSession {
            request_id,
            expected_session_id,
            actual_session_id,
            ..
        } => (
            coco_types::error_codes::PERMISSION_DENIED,
            serde_json::json!({
                "kind": "server_request_wrong_session",
                "request_id": request_id,
                "expected_session_id": expected_session_id,
                "actual_session_id": actual_session_id,
            }),
        ),
        AppServerError::ServerRequestWrongConnection { request_id, .. } => (
            coco_types::error_codes::PERMISSION_DENIED,
            serde_json::json!({ "kind": "server_request_wrong_connection", "request_id": request_id }),
        ),
    };
    JsonRpcDispatchError {
        code,
        message: format!("local AppServer {operation} failed: {error}"),
        data: Some(data),
    }
}

fn attach_lifecycle_error(
    operation: &'static str,
    error: coco_app_server::AttachError,
) -> JsonRpcDispatchError {
    let (code, data) = match &error {
        coco_app_server::AttachError::InteractiveOwnerConflict { session_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "interactive_owner_conflict", "session_id": session_id }),
        ),
        coco_app_server::AttachError::SurfaceLimit { .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "surface_limit" }),
        ),
        coco_app_server::AttachError::SessionClosing { session_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "target_session_not_live", "session_id": session_id, "state": "closing" }),
        ),
    };
    JsonRpcDispatchError {
        code,
        message: format!("local AppServer {operation} failed: {error}"),
        data: Some(data),
    }
}

fn registry_lifecycle_error(
    operation: &'static str,
    error: coco_app_server::RegistryError,
) -> JsonRpcDispatchError {
    use coco_app_server::RegistryError;

    let (code, data) = match &error {
        RegistryError::NotFound { session_id, .. } => (
            coco_types::error_codes::INVALID_PARAMS,
            serde_json::json!({ "kind": "session_not_found", "session_id": session_id }),
        ),
        RegistryError::ResourceExhausted { .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "session_capacity_exhausted" }),
        ),
        RegistryError::OldNotReady { session_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "source_session_not_live", "session_id": session_id }),
        ),
        RegistryError::NewSlotOccupied { session_id, .. } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "destination_session_occupied", "session_id": session_id }),
        ),
        RegistryError::SlotConflict {
            session_id,
            expected,
            ..
        } => (
            coco_types::error_codes::INVALID_REQUEST,
            serde_json::json!({ "kind": "session_slot_conflict", "session_id": session_id, "expected": expected }),
        ),
        RegistryError::LoadFailed { .. } | RegistryError::SignalDropped { .. } => (
            coco_types::error_codes::INTERNAL_ERROR,
            serde_json::json!({ "kind": "session_lifecycle_internal" }),
        ),
    };
    JsonRpcDispatchError {
        code,
        message: format!("local AppServer {operation} failed: {error}"),
        data: Some(data),
    }
}
