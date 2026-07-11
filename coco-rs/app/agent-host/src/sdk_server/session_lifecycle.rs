use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_app_server::AppCloseStart;
use coco_app_server::AppLoadStart;
use coco_app_server::AppServer;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::ConnectionKey;
use coco_app_server::JsonRpcDispatchError;
use coco_app_server::SubscribeReplay;
use coco_app_server::SurfaceRole;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;
use tracing::debug;
use tracing::warn;

use super::app_server_bridge::LocalAppSessionHandle;
use super::handlers::HandlerResult;
use super::handlers::PriorSessionCleanup;
use super::handlers::ReplacementSessionState;
use super::handlers::RuntimeReplacementContext;
use super::handlers::SdkServerState;
use super::handlers::SessionHandoffState;
use super::handlers::SessionMetadata;
use super::handlers::session;

#[derive(Debug, Clone)]
pub(super) enum LocalLifecycleRequest {
    Start { connection: ConnectionKey },
    Resume { connection: ConnectionKey },
    Archive(SessionId),
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
            ClientRequest::SessionArchive(params) => Some(Self::Archive(params.session_id.clone())),
            _ => None,
        }
    }
}

/// Resolve the routed session's runtime handle from the AppServer registry
///. Handlers use this scoped runtime before falling back to the
/// installed singleton, so runtime-control requests target the routed session
/// instead of whichever runtime happens to be installed during a replacement
/// window. Returns `None` for legacy no-registry sessions, which then fall back
/// to the installed slot.
pub(super) fn resolve_scoped_runtime(
    app_server: Option<&Arc<AppServer<LocalAppSessionHandle>>>,
    session_id: Option<&SessionId>,
) -> Option<crate::session_runtime::SessionHandle> {
    app_server?
        .registry()
        .get(session_id?)
        .and_then(LocalAppSessionHandle::into_runtime)
}

pub(super) async fn start_sdk_session_with_scoped_state(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionStartParams,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    if state.has_session_runtime().await {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message:
                "session/start requires AppServer runtime replacement when a runtime is already installed"
                    .to_string(),
            data: None,
        });
    }
    let prepared = session::prepare_session_start(params, &state, false)
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
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let matching_runtime = if let Some(runtime) = state.session_runtime_snapshot().await {
        let runtime_session_id = runtime.current_typed_session_id().await;
        if runtime_session_id != loaded.session_id {
            return Err(JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "session/resume requires AppServer runtime replacement when the installed runtime belongs to a different session".to_string(),
                data: None,
            });
        }
        Some(runtime)
    } else {
        None
    };

    if let Some(runtime) = &matching_runtime {
        session::hydrate_runtime_for_resume_messages(
            runtime,
            &loaded.session_id,
            &loaded.conversation.messages,
        )
        .await;
        runtime.fire_session_start_hooks("resume").await;
        install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
    }

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
    if let Some(runtime) = matching_runtime {
        install_runtime_backed_resumed_sdk_session_state(
            &state,
            &loaded.session,
            loaded.session_id.clone(),
            &runtime,
            &loaded.conversation.messages,
        )
        .await;
    } else {
        session::install_scoped_resumed_session_state(
            &state,
            &loaded.session,
            loaded.session_id.clone(),
            &loaded.conversation.messages,
        )
        .await;
    }
    Ok(result)
}

pub(super) async fn start_sdk_session_with_runtime_replacement(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    params: coco_types::SessionStartParams,
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let prepared = session::prepare_session_start(params, &state, false)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let started_session_id = prepared.session_id.clone();
    let startup_session_id = replacement.startup_session_id.clone();

    let factory = {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let prepared = prepared.clone();
        async move {
            let session_id = prepared.session_id.clone();
            let runtime = build_sdk_runtime_for_start(replacement, state, prepared)
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

    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
    session::install_scoped_started_session_state(
        &state,
        &prepared,
        Some(Arc::clone(runtime.app_state())),
    )
    .await;

    let surface_id =
        attach_local_app_server_surface(&app_server, connection, started_session_id.clone())?;
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
    replacement: RuntimeReplacementContext,
    turn_drain_timeout: Duration,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let resumed_session_id = loaded.session_id.clone();
    let resumed_cwd = loaded.session.working_dir.clone();
    let prior_messages = loaded.conversation.messages.clone();

    if let Some(current_runtime) = state.session_runtime_snapshot().await {
        let current_session_id = current_runtime.current_typed_session_id().await;
        if current_session_id == resumed_session_id {
            session::hydrate_runtime_for_resume_messages(
                &current_runtime,
                &resumed_session_id,
                &prior_messages,
            )
            .await;
            current_runtime.fire_session_start_hooks("resume").await;
            install_sdk_session_runtime_state(Arc::clone(&state), current_runtime.clone()).await;
            install_runtime_backed_resumed_sdk_session_state(
                &state,
                &loaded.session,
                resumed_session_id.clone(),
                &current_runtime,
                &prior_messages,
            )
            .await;
            let surface_id =
                attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
            return encode_session_resume_result(&loaded.session, Some(surface_id));
        }
    }

    let make_factory = || {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let session_id = resumed_session_id.clone();
        let cwd = resumed_cwd.clone();
        let prior_messages = prior_messages.clone();
        async move {
            let runtime = build_sdk_runtime_for_resume(
                replacement,
                state,
                session_id.clone(),
                cwd,
                prior_messages,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(session_id, runtime),
            )
        }
    };

    // resume replaces ONLY the requesting connection's own current
    // session (its sole interactive surface), never a process-wide close-others.
    // With no own current session, this is a plain load into a new slot.
    let own_current = app_server
        .sole_interactive_session_for_connection(connection)
        .filter(|previous| *previous != resumed_session_id);
    let mut replacement_surface_id = None;
    let mut replacement_handle = None;
    if let Some(previous_session_id) = own_current {
        if let Some((handle, surface_id)) = replace_local_app_server_session_with_factory(
            Arc::clone(&app_server),
            Arc::clone(&state),
            previous_session_id.clone(),
            resumed_session_id.clone(),
            make_factory(),
            turn_drain_timeout,
        )
        .await?
        {
            replacement_handle = Some(handle);
            replacement_surface_id = Some(surface_id);
        } else {
            replacement_handle = Some(
                replace_detached_local_app_server_session_with_factory(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    previous_session_id,
                    resumed_session_id.clone(),
                    make_factory(),
                    turn_drain_timeout,
                )
                .await?,
            );
        }
    } else if registered_detached_session(
        &app_server,
        &replacement.startup_session_id,
        &resumed_session_id,
    ) {
        replacement_handle = Some(
            replace_detached_local_app_server_session_with_factory(
                Arc::clone(&app_server),
                Arc::clone(&state),
                replacement.startup_session_id.clone(),
                resumed_session_id.clone(),
                make_factory(),
                turn_drain_timeout,
            )
            .await?,
        );
    }

    let handle = match replacement_handle {
        Some(handle) => handle,
        None => {
            load_local_app_server_session_with_factory(
                &app_server,
                resumed_session_id.clone(),
                make_factory(),
            )
            .await?
        }
    };
    let runtime = handle.require_runtime("resumed")?;
    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
    install_runtime_backed_resumed_sdk_session_state(
        &state,
        &loaded.session,
        resumed_session_id.clone(),
        &runtime,
        &prior_messages,
    )
    .await;

    let surface_id = match replacement_surface_id {
        Some(surface_id) => surface_id,
        None => attach_local_app_server_surface(&app_server, connection, resumed_session_id)?,
    };
    encode_session_resume_result(&loaded.session, Some(surface_id))
}

pub(super) async fn build_sdk_runtime_for_start(
    replacement: RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    prepared: session::PreparedStartSession,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            prepared.session_id.clone(),
            session_build_cwd_from_str(&replacement.cwd, &prepared.cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(&replacement, state, &session).await?;
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
    session_id: SessionId,
    cwd: std::path::PathBuf,
    prior_messages: Vec<coco_messages::Message>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            session_id.clone(),
            session_build_cwd(&replacement.cwd, &cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(&replacement, state, &session).await?;
    session::hydrate_runtime_for_resume_messages(&session, &session_id, &prior_messages).await;
    session.fire_session_start_hooks("resume").await;
    Ok(session)
}

pub(super) async fn setup_sdk_replacement_runtime(
    replacement: &RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    session: &crate::session_runtime::SessionHandle,
) -> anyhow::Result<()> {
    let runtime = session;
    let session_cwd = runtime.original_cwd().clone();
    if replacement.requires_structured_output {
        runtime
            .update_engine_config(|cfg| cfg.requires_structured_output = true)
            .await;
    }
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&state), session);
    if let Some(hooks) = state.sdk_initialize_hooks().await {
        crate::sdk_server::sdk_hooks::register_initialize_hooks(session, &hooks);
    }
    let sdk_agents = state.pending_sdk_agents().await;
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
    let mcp_manager = state.mcp_manager_snapshot().await;
    crate::session_bootstrap::bootstrap_session_mcp(
        session,
        &session_cwd,
        mcp_manager,
        /*await_connect*/ false,
    )
    .await;
    crate::leader_inbox_poller::install_leader(session.clone(), None).await;
    Ok(())
}

pub async fn install_sdk_session_runtime_state(
    state: Arc<SdkServerState>,
    session: crate::session_runtime::SessionHandle,
) {
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&state), &session);
    let runtime = &session;
    let session_manager = Arc::clone(runtime.session_manager());
    let file_history = runtime.file_history().cloned();
    let file_history_config_home = runtime
        .file_history()
        .map(|_| runtime.config_home().clone());
    install_sdk_runtime_reload_subscription(Arc::clone(&state), &session).await;
    state.install_session_runtime(session).await;
    state.install_session_manager(session_manager).await;
    state
        .install_file_history(file_history, file_history_config_home)
        .await;
}

pub(super) async fn install_sdk_runtime_reload_subscription(
    state: Arc<SdkServerState>,
    session: &crate::session_runtime::SessionHandle,
) {
    let runtime = session;
    state.abort_sdk_runtime_reload_subscription().await;

    let Some(sandbox_state) = runtime.sandbox_state() else {
        return;
    };
    sandbox_state.set_approval_bridge(Arc::new(crate::sdk_server::SdkSandboxApprovalBridge::new(
        Arc::clone(&state),
    )));

    let Some(publisher) = runtime.runtime_publisher() else {
        return;
    };
    state
        .install_sdk_runtime_reload_subscription(crate::sandbox_reload::spawn_sandbox_reload(
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
) {
    let plan_mode_instructions = state.pending_plan_mode_instructions().await;
    let runtime = runtime_handle;
    let history = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();

    state.install_scoped_replacement_session_state(ReplacementSessionState {
        session_id: session_id.clone(),
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
        cancel_reason: "runtime-backed session/resume",
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
            // replace ONLY the requesting connection's own current
            // session (its sole interactive surface); never a process-wide
            // close-others. With none, this is a plain register + attach.
            let own_current = app_server
                .sole_interactive_session_for_connection(connection)
                .filter(|previous| *previous != resumed_session_id);
            if let Some(previous_session_id) = own_current {
                if let Some(surface_id) = replace_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    previous_session_id.clone(),
                    LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                    turn_drain_timeout,
                )
                .await?
                {
                    return Ok(Some(surface_id));
                }
                replace_detached_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    previous_session_id,
                    LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                    turn_drain_timeout,
                )
                .await?;
                let surface_id =
                    attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
                return Ok(Some(surface_id));
            }
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
        LocalLifecycleRequest::Archive(session_id) => {
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
        .map_err(|error| local_lifecycle_error("attach session surface", error))?;
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
            params.session_id.clone(),
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
                session_id: params.session_id,
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

pub(super) async fn replace_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_handle: LocalAppSessionHandle,
    turn_drain_timeout: Duration,
) -> Result<Option<SurfaceId>, JsonRpcDispatchError> {
    let new_session_id = new_handle.session_id().clone();
    replace_local_app_server_session_with_factory(
        app_server,
        state,
        old_session_id,
        new_session_id,
        async { Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(new_handle) },
        turn_drain_timeout,
    )
    .await
    .map(|replacement| replacement.map(|(_, surface)| surface))
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
        .map_err(|error| local_lifecycle_error("replace session", error))?
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
        .map_err(|error| local_lifecycle_error("replace detached session", error))?
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
            .close_if_current_session(handle.session_id(), reason)
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

    // also clear the installed process singletons when this closing id
    // is the one they back, so an archived / idle-swept session id can no longer
    // receive stamps, hub egress, or "successful" control mutations against a
    // shut-down runtime. The match is gated by id, so a concurrent replacement
    // swap that already installed a different runtime is left untouched.
    state
        .clear_installed_singletons_if_matches(session_id)
        .await;
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
            tracing::debug!(%error, "failed to persist session_seq watermark at close");
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
