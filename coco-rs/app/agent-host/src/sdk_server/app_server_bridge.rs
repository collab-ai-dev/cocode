//! Bridge from the new AppServer JSON-RPC adapter to the existing SDK handlers.
//!
//! This keeps runtime semantics in `coco-agent-host` while allowing
//! `coco-app-server` to own JSON-RPC connection dispatch and routing.

use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use crate::local_client::{LocalServerClient, LocalSessionClient};
use coco_app_server::{
    AppServer, AttachSurfaceOptions, DisconnectOutcome, JsonRpcAdapterConnection,
    JsonRpcAdapterError, JsonRpcDispatchError, JsonRpcRequestContext, JsonRpcRequestFuture,
    JsonRpcRequestHandler, LocalClientAdapter, LocalClientRequestContext, LocalClientRequestFuture,
    LocalClientRequestHandler, SurfaceLimits,
};
use coco_app_server_client::ClientError;
use coco_app_server_transport::JsonRpcFrame;
use coco_hub_connector::HubConnectorSender;
use coco_types::{
    ClientRequest, CoreEvent, ServerNotification, SessionEnvelope, SessionId, SurfaceId,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::warn;

use super::session_lifecycle::*;
use crate::sdk_server::{
    dispatcher::spawn_sdk_outbound_writer,
    handlers::{
        HandlerContext, HandlerResult, SdkServerState, SessionHandoffState, SessionMetadata,
        dispatch_client_request, session,
    },
    outbound::{OutboundMessage, ProcessEvent, event_agent_id},
    session_data::{LocalSessionDataRequest, LocalSessionDataView},
    transport::{SdkTransport, TransportError},
};

const APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY: usize = 128;
const APP_SERVER_LOCAL_CHANNEL_CAPACITY: usize = 128;
const APP_SERVER_LOCAL_RETENTION_PER_SESSION: usize = 128;
const APP_SERVER_MAX_SURFACES_PER_CONNECTION: usize = 8;
const APP_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION: usize = 16;
pub(crate) const APP_SERVER_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

fn server_config_usize(value: i64, fallback: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn server_config_surface_limits(server_config: &coco_config::ServerConfig) -> SurfaceLimits {
    SurfaceLimits {
        max_surfaces_per_connection: server_config_usize(
            server_config.max_surfaces_per_connection,
            APP_SERVER_MAX_SURFACES_PER_CONNECTION,
        ),
        max_passive_surfaces_per_session: server_config_usize(
            server_config.max_passive_surfaces_per_session,
            APP_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION,
        ),
    }
}

fn server_config_duration_secs(value: i64, fallback: Duration) -> Duration {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

#[derive(Debug, Clone)]
pub struct AppServerLocalTurnCompletion {
    pub started: coco_types::TurnStartResult,
    pub ended: coco_types::TurnEndedParams,
}

/// Runtime-backed request handler for AppServer adapters.
#[derive(Clone)]
pub struct AppServerSdkHandler {
    state: Arc<SdkServerState>,
    notif_tx: mpsc::Sender<OutboundMessage>,
    local_app_server: Option<Arc<AppServer<LocalAppSessionHandle>>>,
    turn_drain_timeout: Duration,
    connection_profile: Arc<std::sync::OnceLock<coco_types::ConnectionProfile>>,
    require_initialize: bool,
}

/// Local AppServer registry handle for the current application-host bridge.
///
/// The registry id is an immutable snapshot that is checked against the
/// optional runtime handle during close cascades. Runtime replacement installs a
/// fresh handle instead of mutating an existing handle in place.
#[derive(Clone)]
pub struct LocalAppSessionHandle {
    session_id: SessionId,
    runtime: Option<crate::session_runtime::SessionHandle>,
}

impl LocalAppSessionHandle {
    pub(super) fn snapshot(session_id: SessionId) -> Self {
        Self {
            session_id,
            runtime: None,
        }
    }

    pub fn from_runtime(
        session_id: SessionId,
        runtime: crate::session_runtime::SessionHandle,
    ) -> Self {
        Self {
            session_id,
            runtime: Some(runtime),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub(super) fn runtime(&self) -> Option<&crate::session_runtime::SessionHandle> {
        self.runtime.as_ref()
    }

    pub(super) fn has_runtime(&self) -> bool {
        self.runtime.is_some()
    }

    pub fn into_session(self) -> Option<crate::session_runtime::SessionHandle> {
        self.runtime
    }

    pub(super) fn require_runtime(
        self,
        action: &str,
    ) -> Result<crate::session_runtime::SessionHandle, JsonRpcDispatchError> {
        let session_id = self.session_id.clone();
        self.into_session().ok_or_else(|| JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!(
                "local AppServer session {session_id} {action} without a runtime handle"
            ),
            data: None,
        })
    }

    pub fn require_runtime_anyhow(
        self,
        action: &str,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle> {
        let session_id = self.session_id.clone();
        self.into_session().ok_or_else(|| {
            anyhow::anyhow!(
                "local AppServer session {session_id} {action} without a runtime handle"
            )
        })
    }

    pub(crate) async fn live_summary_and_history(
        &self,
    ) -> Option<(
        coco_types::SdkSessionSummary,
        Vec<Arc<coco_messages::Message>>,
    )> {
        let runtime = self.runtime()?;
        let current_session_id = runtime.current_typed_session_id().await;
        if current_session_id.as_str() != self.session_id.as_str() {
            return None;
        }

        let config = runtime.current_engine_config().await;
        let history = runtime.history().lock().await.to_vec();
        let usage = runtime.session_usage_snapshot().await;
        let timestamp = chrono::Utc::now().to_rfc3339();
        Some((
            coco_types::SdkSessionSummary {
                session_id: self.session_id.clone(),
                model: config.model_id,
                cwd: runtime.original_cwd().to_string_lossy().into_owned(),
                created_at: timestamp.clone(),
                updated_at: Some(timestamp),
                title: None,
                message_count: history.len() as i32,
                total_tokens: usage
                    .totals
                    .input_tokens
                    .saturating_add(usage.totals.output_tokens),
            },
            history,
        ))
    }
}

impl AppServerSdkHandler {
    pub fn new(state: Arc<SdkServerState>, notif_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: None,
            turn_drain_timeout: APP_SERVER_TURN_DRAIN_TIMEOUT,
            connection_profile: Arc::new(std::sync::OnceLock::new()),
            require_initialize: true,
        }
    }

    pub fn with_local_app_server(
        state: Arc<SdkServerState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<LocalAppSessionHandle>>,
    ) -> Self {
        Self::with_local_app_server_and_turn_drain_timeout(
            state,
            notif_tx,
            app_server,
            APP_SERVER_TURN_DRAIN_TIMEOUT,
        )
    }

    pub fn with_local_app_server_and_turn_drain_timeout(
        state: Arc<SdkServerState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<LocalAppSessionHandle>>,
        turn_drain_timeout: Duration,
    ) -> Self {
        let connection_profile = Arc::new(std::sync::OnceLock::new());
        let profile = match coco_types::ConnectionProfile::try_from(
            coco_types::InitializeParams::default(),
        ) {
            Ok(profile) => profile,
            Err(error) => panic!("invalid built-in local connection profile: {error}"),
        };
        let _ = connection_profile.set(profile);
        Self {
            state,
            notif_tx,
            local_app_server: Some(app_server),
            turn_drain_timeout,
            connection_profile,
            require_initialize: false,
        }
    }

    fn profile_for_request(
        &self,
        request: &ClientRequest,
    ) -> Result<Arc<coco_types::ConnectionProfile>, JsonRpcDispatchError> {
        if let ClientRequest::Initialize(params) = request {
            if self.connection_profile.get().is_some() {
                return Err(JsonRpcDispatchError {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "connection is already initialized".to_string(),
                    data: Some(serde_json::json!({ "kind": "already_initialized" })),
                });
            }
            let profile =
                coco_types::ConnectionProfile::try_from(params.clone()).map_err(|error| {
                    JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_PARAMS,
                        message: error.to_string(),
                        data: None,
                    }
                })?;
            self.connection_profile
                .set(profile)
                .map_err(|_| JsonRpcDispatchError {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "connection is already initialized".to_string(),
                    data: Some(serde_json::json!({ "kind": "already_initialized" })),
                })?;
        }
        self.connection_profile
            .get()
            .cloned()
            .map(Arc::new)
            .ok_or_else(|| JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: if self.require_initialize {
                    "connection is not initialized"
                } else {
                    "local connection profile is unavailable"
                }
                .to_string(),
                data: Some(serde_json::json!({ "kind": "not_initialized" })),
            })
    }
}

impl JsonRpcRequestHandler for AppServerSdkHandler {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let connection_profile = match self.profile_for_request(&request) {
            Ok(profile) => profile,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let local_app_server = self.local_app_server.clone();
        if let (Some(app_server), ClientRequest::SessionSubscribe(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection;
            return Box::pin(async move {
                subscribe_local_app_server_session(app_server, connection, params).await
            });
        }
        if let (Some(app_server), ClientRequest::CancelRequest(params)) =
            (local_app_server.clone(), &request)
        {
            let request_id = coco_types::RequestId::String(params.request_id.clone());
            let connection = context.connection;
            return Box::pin(async move {
                app_server
                    .cancel_server_request_for_connection(connection, &request_id)
                    .map(|_| serde_json::Value::Null)
                    .map_err(|error| JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: error.to_string(),
                        data: Some(serde_json::json!({
                            "kind": "pending_request_mismatch",
                            "request_id": request_id,
                        })),
                    })
            });
        }
        let lifecycle_request = local_app_server
            .as_ref()
            .and_then(|_| LocalLifecycleRequest::from_client_request(context.connection, &request));
        let session_data_request = local_app_server
            .as_ref()
            .and_then(|_| LocalSessionDataRequest::from_client_request(&request));
        if let (Some(app_server), Some(session_data_request)) =
            (local_app_server.clone(), session_data_request)
        {
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                LocalSessionDataView { app_server, state }
                    .handle(&session_data_request)
                    .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionStart(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.as_ref().clone();
            let connection = context.connection;
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await;
                if let Some(replacement) = replacement {
                    return start_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        params,
                        Arc::clone(&connection_profile),
                        replacement,
                        turn_drain_timeout,
                    )
                    .await;
                }
                start_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    params,
                    Arc::clone(&connection_profile),
                    turn_drain_timeout,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionResume(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection;
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await;
                if let Some(replacement) = replacement {
                    return resume_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        params,
                        Arc::clone(&connection_profile),
                        replacement,
                        turn_drain_timeout,
                    )
                    .await;
                }
                resume_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    params,
                    Arc::clone(&connection_profile),
                    turn_drain_timeout,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionReplace(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.as_ref().clone();
            let connection = context.connection;
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await.ok_or_else(|| {
                    JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: "session/replace requires a runtime factory".to_string(),
                        data: None,
                    }
                })?;
                replace_sdk_session_with_runtime(
                    app_server,
                    state,
                    connection,
                    params,
                    connection_profile,
                    replacement,
                    turn_drain_timeout,
                )
                .await
            });
        }
        let (target_session_id, session) = match resolve_request_runtime(
            local_app_server.as_ref(),
            context.connection,
            &request,
        ) {
            Ok(resolved) => resolved,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
            connection_profile,
            app_server: local_app_server.clone(),
            target_session_id,
            session,
        };
        let state = Arc::clone(&self.state);
        let turn_drain_timeout = self.turn_drain_timeout;
        Box::pin(async move {
            let dispatch_result = dispatch_sdk_client_request(request, ctx).await;
            let mut result = dispatch_result?;
            if let (Some(app_server), Some(lifecycle_request)) =
                (local_app_server, lifecycle_request)
                && let Some(surface_id) = apply_local_lifecycle_request(
                    app_server,
                    Arc::clone(&state),
                    lifecycle_request,
                    &result,
                    turn_drain_timeout,
                )
                .await?
            {
                inject_surface_id(&mut result, surface_id)?;
            }
            Ok(result)
        })
    }
}

impl LocalClientRequestHandler for AppServerSdkHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let connection_profile = match self.profile_for_request(&request) {
            Ok(profile) => profile,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let local_app_server = self.local_app_server.clone();
        if let (Some(app_server), ClientRequest::SessionSubscribe(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection_key();
            return Box::pin(async move {
                subscribe_local_app_server_session(app_server, connection, params).await
            });
        }
        if let (Some(app_server), ClientRequest::CancelRequest(params)) =
            (local_app_server.clone(), &request)
        {
            let request_id = coco_types::RequestId::String(params.request_id.clone());
            let connection = context.connection_key();
            return Box::pin(async move {
                app_server
                    .cancel_server_request_for_connection(connection, &request_id)
                    .map(|_| serde_json::Value::Null)
                    .map_err(|error| JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: error.to_string(),
                        data: Some(serde_json::json!({
                            "kind": "pending_request_mismatch",
                            "request_id": request_id,
                        })),
                    })
            });
        }
        let lifecycle_request = local_app_server.as_ref().and_then(|_| {
            LocalLifecycleRequest::from_client_request(context.connection_key(), &request)
        });
        let session_data_request = local_app_server
            .as_ref()
            .and_then(|_| LocalSessionDataRequest::from_client_request(&request));
        if let (Some(app_server), Some(session_data_request)) =
            (local_app_server.clone(), session_data_request)
        {
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                LocalSessionDataView { app_server, state }
                    .handle(&session_data_request)
                    .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionStart(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.as_ref().clone();
            let connection = context.connection_key();
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await;
                if let Some(replacement) = replacement {
                    return start_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        params,
                        Arc::clone(&connection_profile),
                        replacement,
                        turn_drain_timeout,
                    )
                    .await;
                }
                start_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    params,
                    Arc::clone(&connection_profile),
                    turn_drain_timeout,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionResume(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection_key();
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await;
                if let Some(replacement) = replacement {
                    return resume_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        params,
                        Arc::clone(&connection_profile),
                        replacement,
                        turn_drain_timeout,
                    )
                    .await;
                }
                resume_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    params,
                    Arc::clone(&connection_profile),
                    turn_drain_timeout,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionReplace(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.as_ref().clone();
            let connection = context.connection_key();
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement = state.runtime_replacement_snapshot().await.ok_or_else(|| {
                    JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: "session/replace requires a runtime factory".to_string(),
                        data: None,
                    }
                })?;
                replace_sdk_session_with_runtime(
                    app_server,
                    state,
                    connection,
                    params,
                    connection_profile,
                    replacement,
                    turn_drain_timeout,
                )
                .await
            });
        }
        let (target_session_id, session) = match resolve_request_runtime(
            local_app_server.as_ref(),
            context.connection_key(),
            &request,
        ) {
            Ok(resolved) => resolved,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
            connection_profile,
            app_server: local_app_server.clone(),
            target_session_id,
            session,
        };
        let state = Arc::clone(&self.state);
        let turn_drain_timeout = self.turn_drain_timeout;
        Box::pin(async move {
            let dispatch_result = dispatch_sdk_client_request(request, ctx).await;
            let mut result = dispatch_result?;
            if let (Some(app_server), Some(lifecycle_request)) =
                (local_app_server, lifecycle_request)
                && let Some(surface_id) = apply_local_lifecycle_request(
                    app_server,
                    Arc::clone(&state),
                    lifecycle_request,
                    &result,
                    turn_drain_timeout,
                )
                .await?
            {
                inject_surface_id(&mut result, surface_id)?;
            }
            Ok(result)
        })
    }
}

impl coco_app_server::JsonRpcConnectionHandlerFactory for AppServerSdkHandler {
    type Handler = Self;

    fn open(&self, _connection: coco_app_server::ConnectionKey) -> Arc<Self::Handler> {
        Arc::new(Self {
            state: Arc::clone(&self.state),
            notif_tx: self.notif_tx.clone(),
            local_app_server: self.local_app_server.clone(),
            turn_drain_timeout: self.turn_drain_timeout,
            connection_profile: Arc::new(std::sync::OnceLock::new()),
            require_initialize: true,
        })
    }
}

fn resolve_request_runtime(
    app_server: Option<&Arc<AppServer<LocalAppSessionHandle>>>,
    connection: coco_app_server::ConnectionKey,
    request: &ClientRequest,
) -> Result<
    (
        Option<SessionId>,
        Option<crate::sdk_server::handlers::SessionRequestContext>,
    ),
    JsonRpcDispatchError,
> {
    if let ClientRequest::SessionArchive(params) = request {
        return Ok((Some(params.target.session_id().clone()), None));
    }
    let Some(target) = request.interactive_target() else {
        let Some(target) = request.session_target() else {
            return Ok((None, None));
        };
        let runtime = app_server
            .and_then(|server| server.registry().get(&target.session_id))
            .and_then(LocalAppSessionHandle::into_session);
        return Ok((
            Some(target.session_id.clone()),
            runtime.map(
                |runtime| crate::sdk_server::handlers::SessionRequestContext {
                    session_id: target.session_id.clone(),
                    runtime,
                },
            ),
        ));
    };
    let app_server = app_server.ok_or_else(|| JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: "interactive request requires AppServer routing".to_string(),
        data: None,
    })?;
    let validated = app_server
        .validate_interactive_target(connection, target)
        .map_err(|error| {
            crate::sdk_server::session_lifecycle::app_server_lifecycle_error(
                "resolve request target",
                error,
            )
        })?;
    let runtime = validated
        .handle
        .runtime()
        .cloned()
        .ok_or_else(|| JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!(
                "targeted AppServer session {} has no runtime handle",
                target.session_id
            ),
            data: None,
        })?;
    Ok((
        Some(target.session_id.clone()),
        Some(crate::sdk_server::handlers::SessionRequestContext {
            session_id: target.session_id.clone(),
            runtime,
        }),
    ))
}

pub struct AppServerLocalBridge {
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    client: LocalServerClient<LocalAppSessionHandle>,
    handler: AppServerSdkHandler,
    outbound_forwarder: JoinHandle<()>,
    hub_connector: Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
    event_pump: Option<JoinHandle<()>>,
    event_pump_session_id: Option<SessionId>,
    interactive_surface: Option<LocalSessionClient>,
    channel_capacity: usize,
}

impl AppServerLocalBridge {
    pub fn interactive_session(&self) -> Option<&LocalSessionClient> {
        self.interactive_surface.as_ref()
    }

    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self::with_channel_capacity(state, APP_SERVER_LOCAL_CHANNEL_CAPACITY)
    }

    pub fn with_server_config(
        state: Arc<SdkServerState>,
        server_config: &coco_config::ServerConfig,
    ) -> Self {
        Self::with_capacity_and_retention(
            state,
            server_config_usize(
                server_config.outbound_queue_frames,
                APP_SERVER_LOCAL_CHANNEL_CAPACITY,
            ),
            server_config_usize(
                server_config.event_retention_per_session,
                APP_SERVER_LOCAL_RETENTION_PER_SESSION,
            ),
            server_config_surface_limits(server_config),
            server_config_duration_secs(
                server_config.turn_drain_timeout_secs,
                APP_SERVER_TURN_DRAIN_TIMEOUT,
            ),
        )
    }

    pub fn with_hub_connector_sender(
        state: Arc<SdkServerState>,
        hub_connector: HubConnectorSender,
    ) -> Self {
        Self::with_channel_capacity_and_hub_connector(
            state,
            APP_SERVER_LOCAL_CHANNEL_CAPACITY,
            Some(hub_connector),
        )
    }

    pub fn with_channel_capacity(state: Arc<SdkServerState>, channel_capacity: usize) -> Self {
        Self::with_capacity_retention_and_hub_connector(
            state,
            channel_capacity,
            channel_capacity,
            SurfaceLimits::default(),
            APP_SERVER_TURN_DRAIN_TIMEOUT,
            None,
        )
    }

    fn with_capacity_and_retention(
        state: Arc<SdkServerState>,
        channel_capacity: usize,
        event_retention_per_session: usize,
        surface_limits: SurfaceLimits,
        turn_drain_timeout: Duration,
    ) -> Self {
        Self::with_capacity_retention_and_hub_connector(
            state,
            channel_capacity,
            event_retention_per_session,
            surface_limits,
            turn_drain_timeout,
            None,
        )
    }

    fn with_channel_capacity_and_hub_connector(
        state: Arc<SdkServerState>,
        channel_capacity: usize,
        hub_connector: Option<HubConnectorSender>,
    ) -> Self {
        Self::with_capacity_retention_and_hub_connector(
            state,
            channel_capacity,
            channel_capacity,
            SurfaceLimits::default(),
            APP_SERVER_TURN_DRAIN_TIMEOUT,
            hub_connector,
        )
    }

    fn with_capacity_retention_and_hub_connector(
        state: Arc<SdkServerState>,
        channel_capacity: usize,
        event_retention_per_session: usize,
        surface_limits: SurfaceLimits,
        turn_drain_timeout: Duration,
        hub_connector: Option<HubConnectorSender>,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "local AppServer bridge channel capacity must be non-zero"
        );
        assert!(
            event_retention_per_session > 0,
            "local AppServer bridge event retention must be non-zero"
        );
        let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new_with_surface_limits(
            /*max_sessions*/ 1,
            event_retention_per_session,
            surface_limits,
        ));
        install_session_seq_durability(&state, event_retention_per_session as i64);
        let adapter =
            LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), channel_capacity);
        let client = LocalServerClient::connect_local(&adapter);
        let (outbound_tx, outbound_rx) = mpsc::channel(channel_capacity);
        let handler = AppServerSdkHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        );
        let hub_connector = Arc::new(std::sync::RwLock::new(hub_connector));
        let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
            Arc::clone(&app_server),
            state,
            outbound_rx,
            Arc::clone(&hub_connector),
        );
        Self {
            app_server,
            client,
            handler,
            outbound_forwarder,
            hub_connector,
            event_pump: None,
            event_pump_session_id: None,
            interactive_surface: None,
            channel_capacity,
        }
    }

    pub fn app_server(&self) -> &Arc<AppServer<LocalAppSessionHandle>> {
        &self.app_server
    }

    pub fn client(&self) -> &LocalServerClient<LocalAppSessionHandle> {
        &self.client
    }

    pub fn set_hub_connector_sender(&self, sender: HubConnectorSender) {
        match self.hub_connector.write() {
            Ok(mut guard) => *guard = Some(sender),
            Err(poisoned) => *poisoned.into_inner() = Some(sender),
        }
    }

    pub fn client_mut(&mut self) -> &mut LocalServerClient<LocalAppSessionHandle> {
        &mut self.client
    }

    pub fn connect_local_client(&self) -> LocalServerClient<LocalAppSessionHandle> {
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        LocalServerClient::connect_local(&adapter)
    }

    pub fn handler(&self) -> &AppServerSdkHandler {
        &self.handler
    }

    pub async fn close_registered_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        close_local_app_server_session(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            session_id,
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn shutdown_registered_sessions(&self) -> Result<(), ClientError> {
        shutdown_local_app_server_sessions(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn load_session_runtime<F>(
        &self,
        session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = session_id.clone();
        let handle = load_local_app_server_session_with_factory(
            &self.app_server,
            session_id.clone(),
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                    LocalAppSessionHandle::from_runtime(registry_session_id, runtime),
                )
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        handle.require_runtime_anyhow("loaded")
    }

    pub async fn load_session_runtime_from_factory(
        &self,
        session_id: SessionId,
        runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle> {
        let handle =
            load_local_app_server_session_runtime(&self.app_server, session_id, runtime_factory)
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        handle.require_runtime_anyhow("loaded")
    }

    pub async fn load_session_runtime_from_factory_with_cwd(
        &self,
        session_id: SessionId,
        runtime_factory: crate::session_runtime::SessionRuntimeFactory,
        cwd: PathBuf,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle> {
        let handle = load_local_app_server_session_runtime_with_cwd(
            &self.app_server,
            session_id,
            runtime_factory,
            cwd,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        handle.require_runtime_anyhow("loaded")
    }

    pub async fn replace_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<Option<(crate::session_runtime::SessionHandle, SurfaceId)>>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        replace_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
            self.handler.turn_drain_timeout,
        )
        .await
        .and_then(|replacement| {
            replacement
                .map(|(handle, surface_id)| {
                    handle
                        .require_runtime("replaced")
                        .map(|runtime| (runtime, surface_id))
                })
                .transpose()
        })
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub async fn replace_detached_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        replace_detached_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
            self.handler.turn_drain_timeout,
        )
        .await
        .and_then(|handle| handle.require_runtime("replaced"))
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub fn ensure_interactive_surface(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        if self
            .interactive_surface
            .as_ref()
            .is_some_and(|surface| surface.session_id() == &session_id)
        {
            return Ok(());
        }
        let can_repoint = self.interactive_surface.as_ref().is_some_and(|surface| {
            self.surface_is_attached_to_session(surface.surface_id(), &session_id)
        });
        if can_repoint {
            // Consume the old handle and mint the successor on the same surface;
            // handles are never re-pointed in place.
            self.interactive_surface = self
                .interactive_surface
                .take()
                .map(|surface| surface.into_replaced(session_id));
            return Ok(());
        }
        let surface = self
            .client
            .attach_interactive_session(session_id, AttachSurfaceOptions::default())?;
        self.interactive_surface = Some(surface);
        Ok(())
    }

    fn surface_is_attached_to_session(
        &self,
        surface_id: &SurfaceId,
        session_id: &SessionId,
    ) -> bool {
        self.surface_session_id(surface_id).as_ref() == Some(session_id)
    }

    fn surface_session_id(&self, surface_id: &SurfaceId) -> Option<SessionId> {
        let routing = self
            .app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        routing.surface_session(surface_id).cloned()
    }

    pub fn start_passive_event_pump(
        &mut self,
        session_id: SessionId,
        event_tx: mpsc::Sender<CoreEvent>,
    ) -> Result<(), ClientError> {
        if self
            .event_pump_session_id
            .as_ref()
            .is_some_and(|active| active == &session_id)
        {
            return Ok(());
        }
        if let Some(handle) = self.event_pump.take() {
            handle.abort();
        }
        self.event_pump_session_id = None;
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        let mut connection = adapter.connect();
        let surface = connection
            .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
            .map_err(crate::local_client::client_error_from_attach)?;
        let surface_id = surface.surface_id;
        self.event_pump = Some(tokio::spawn(async move {
            while let Some(delivery) = connection.events_mut().recv().await {
                if delivery.surface_id == surface_id
                    && event_tx.send(delivery.envelope.event).await.is_err()
                {
                    break;
                }
            }
        }));
        self.event_pump_session_id = Some(session_id);
        Ok(())
    }

    pub async fn drain_interactive_events_to(&mut self, event_tx: &mpsc::Sender<CoreEvent>) {
        let Some(surface) = self.interactive_surface.clone() else {
            return;
        };
        for pass in 0..2 {
            while let Some(envelope) = self.client.try_next_session_event(&surface) {
                if event_tx.send(envelope.event).await.is_err() {
                    return;
                }
            }
            if pass == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    pub async fn start_turn_and_wait_for_end(
        &mut self,
        session_id: SessionId,
        mut params: coco_types::TurnStartParams,
    ) -> Result<AppServerLocalTurnCompletion, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        params.target = surface.interactive_target();
        let handler = self.handler.clone();
        let started = self.client.turn_start(&handler, params).await?;
        loop {
            let Some(envelope) = self.client.next_session_event(&surface).await else {
                return Err(ClientError::Disconnected);
            };
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == started.turn_id
            {
                return Ok(AppServerLocalTurnCompletion { started, ended });
            }
        }
    }

    pub async fn start_turn(
        &mut self,
        session_id: SessionId,
        mut params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .as_ref()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        params.target = surface.interactive_target();
        self.client.turn_start(&self.handler, params).await
    }

    pub async fn current_session_result(&self) -> Option<coco_types::SessionResultParams> {
        let session_id = if let Some(surface) = self.interactive_surface.as_ref()
            && let Some(session_id) = self.surface_session_id(surface.surface_id())
        {
            session_id
        } else {
            return None;
        };
        Some(session::build_aggregated_session_result(
            &session_id,
            self.handler.state.as_ref(),
            "end_turn",
        ))
    }

    pub async fn replace_session_runtime_for_clear<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<Option<(crate::session_runtime::SessionHandle, SurfaceId)>>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        let replacement = replace_local_app_server_session_with_factory_and_close_reason(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
            coco_hooks::orchestration::ExitReason::Clear,
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;

        match replacement {
            Some((handle, surface_id)) => {
                let runtime = handle.require_runtime_anyhow("replaced")?;
                Ok(Some((runtime, surface_id)))
            }
            None => Ok(None),
        }
    }

    pub async fn install_session_snapshot(
        &self,
        session_id: SessionId,
        cwd: impl Into<String>,
        model: impl Into<String>,
    ) {
        let cwd = cwd.into();
        let model = model.into();
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            LocalAppSessionHandle::snapshot(session_id.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry snapshot install failed");
        }
        self.handler
            .state
            .set_session_metadata(session_id.clone(), SessionMetadata { cwd, model });
        self.handler
            .state
            .set_session_handoff(session_id.clone(), SessionHandoffState::new());
        self.handler
            .state
            .clear_session_plan_mode_instructions(&session_id);
        self.handler
            .state
            .reset_session_accounting(session_id.clone());
    }

    pub async fn install_session_runtime(&self, session: crate::session_runtime::SessionHandle) {
        crate::sdk_server::sdk_hooks::install_runtime_callback(
            Arc::clone(&self.app_server),
            &session,
        );
        let (
            session_id,
            cwd,
            model,
            bypass_permissions_available,
            app_state,
            history,
            session_manager,
        ) = {
            let runtime = &session;
            let session_id = session.session_id().clone();
            let cwd = runtime
                .current_cwd()
                .read()
                .await
                .to_string_lossy()
                .into_owned();
            let config = runtime.current_engine_config().await;
            let history = runtime.history().lock().await.iter().cloned().collect();
            (
                session_id,
                cwd,
                config.model_id.clone(),
                config.permission_mode_availability.bypass_permissions,
                Arc::clone(runtime.app_state()),
                history,
                Arc::clone(runtime.session_manager()),
            )
        };
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            LocalAppSessionHandle::from_runtime(session_id.clone(), session.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry install failed");
        }

        self.handler
            .state
            .set_session_metadata(session_id.clone(), SessionMetadata { cwd, model });
        self.handler.state.set_session_handoff(
            session_id.clone(),
            SessionHandoffState {
                history: Arc::new(tokio::sync::Mutex::new(history)),
                app_state,
            },
        );
        self.handler
            .state
            .clear_session_plan_mode_instructions(&session_id);
        self.handler
            .state
            .reset_session_accounting(session_id.clone());
        self.handler
            .state
            .set_bypass_permissions_available(bypass_permissions_available);
        self.handler
            .state
            .install_turn_runner(Arc::new(crate::sdk_server::SessionTurnExecutor::new(
                None, None,
            )))
            .await;
        self.handler
            .state
            .install_session_manager(session_manager)
            .await;
    }
}

impl Drop for AppServerLocalBridge {
    fn drop(&mut self) {
        self.outbound_forwarder.abort();
        if let Some(handle) = &self.event_pump {
            handle.abort();
        }
    }
}

pub fn spawn_app_server_local_outbound_forwarder<H>(
    server: Arc<AppServer<H>>,
    state: Arc<SdkServerState>,
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    hub_connector: Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> JoinHandle<()>
where
    H: Clone + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let session_seq = Arc::clone(state.session_seq_allocator());
        while let Some(outbound) = outbound_rx.recv().await {
            match outbound {
                OutboundMessage::SessionEvent {
                    session_id,
                    event,
                    routed,
                } => {
                    let hub_connector = clone_hub_connector_sender(&hub_connector);
                    route_local_outbound_event(
                        &server,
                        hub_connector.as_ref(),
                        &session_seq,
                        session_id,
                        *event,
                    );
                    if let Some(routed) = routed {
                        let _ = routed.send(());
                    }
                }
                OutboundMessage::ProcessEvent(_) => {
                    warn!("dropping process event on local AppServer forwarder");
                }
                OutboundMessage::JsonRpcFrame(_) => {
                    warn!("dropping JSON-RPC outbound message on local AppServer forwarder");
                }
            }
        }
    })
}

/// Configure the process-shared durable `session_seq` allocator:
/// bind the skip-ahead window to the retention ring size and install a
/// best-effort persist hook that appends each due watermark to the session's
/// transcript. Idempotent — repeated setup only re-binds the window and hook.
pub fn install_session_seq_durability(state: &Arc<SdkServerState>, event_retention: i64) {
    let allocator = state.session_seq_allocator();
    allocator.set_skip_ahead_window(event_retention);
    // Weak reference so the hook never keeps `SdkServerState` (which owns the
    // allocator) alive — otherwise state -> allocator -> hook -> state leaks.
    let weak_state = Arc::downgrade(state);
    allocator.set_persist_hook(Arc::new(move |session_id, session_seq| {
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let session_id = session_id.clone();
        // The hook fires from inside the forwarder task (a Tokio context), so
        // resolving the manager and writing the transcript can be deferred off
        // the routing path.
        tokio::spawn(async move {
            let Some(manager) = state.session_manager_snapshot().await else {
                return;
            };
            let id = session_id.as_str().to_string();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(error) = manager.persist_session_seq_watermark(&id, session_seq) {
                    tracing::debug!(%error, "failed to persist session_seq watermark");
                }
            })
            .await;
        });
    }));
}

fn clone_hub_connector_sender(
    hub_connector: &Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> Option<HubConnectorSender> {
    match hub_connector.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn route_local_outbound_event<H>(
    server: &AppServer<H>,
    hub_connector: Option<&HubConnectorSender>,
    session_seq: &coco_app_server::SessionSeqAllocator,
    session_id: SessionId,
    event: CoreEvent,
) where
    H: Clone + Send + Sync + 'static,
{
    let seq_session_id = session_id.clone();
    let agent_id = event_agent_id(&event);
    let envelope = SessionEnvelope::stamp(session_id, agent_id, event, || {
        session_seq.next(&seq_session_id)
    });
    let hub_envelope = envelope.clone();
    server.route_envelope(envelope);
    if let Some(hub_connector) = hub_connector
        && let Err(error) = hub_connector.try_enqueue(hub_envelope)
    {
        warn!(%error, "dropping local AppServer event from Hub connector queue");
    }
}

pub async fn dispatch_sdk_client_request(
    request: ClientRequest,
    ctx: HandlerContext,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    match dispatch_client_request(request, ctx).await {
        HandlerResult::Ok(result) => Ok(result),
        HandlerResult::Err {
            code,
            message,
            data,
        } => Err(JsonRpcDispatchError {
            code,
            message,
            data,
        }),
        HandlerResult::NotImplemented(method) => {
            Err(JsonRpcDispatchError::method_not_found(method))
        }
    }
}

pub async fn run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
    connection: JsonRpcAdapterConnection<LocalAppSessionHandle>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
    hub_connector: Option<HubConnectorSender>,
    turn_drain_timeout: Duration,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError> {
    let app_server = connection.app_server();
    let (outbound_tx, outbound_rx) = mpsc::channel::<OutboundMessage>(256);
    let mut external_forwarders = Vec::new();
    for mut rx in external_notifications {
        let forwarded_tx = outbound_tx.clone();
        external_forwarders.push(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let Some(event) = ProcessEvent::from_core_event(event) else {
                    warn!("dropping session-scoped event from SDK process-event source");
                    continue;
                };
                if forwarded_tx
                    .send(OutboundMessage::ProcessEvent(event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    let writer_task = spawn_sdk_outbound_writer(
        Arc::clone(&transport),
        outbound_rx,
        Arc::clone(&app_server),
        Arc::clone(state.session_seq_allocator()),
        hub_connector,
    );
    let handler_factory = AppServerSdkHandler::with_local_app_server_and_turn_drain_timeout(
        Arc::clone(&state),
        outbound_tx.clone(),
        Arc::clone(&app_server),
        turn_drain_timeout,
    );
    let handler = coco_app_server::JsonRpcConnectionHandlerFactory::open(
        &handler_factory,
        connection.connection_key(),
    );
    let result = run_app_server_connection_over_sdk_transport_inner(
        connection,
        transport,
        handler,
        Some(outbound_tx.clone()),
    )
    .await;
    drop(handler_factory);

    for forwarder in external_forwarders {
        forwarder.abort();
        let _ = forwarder.await;
    }
    drop(outbound_tx);
    writer_task.await.map_err(SdkAppServerBridgeError::join)?;
    result
}

async fn run_app_server_connection_over_sdk_transport_inner<H, Handler>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    handler: Arc<Handler>,
    outbound_messages: Option<mpsc::Sender<OutboundMessage>>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
    Handler: JsonRpcRequestHandler,
{
    let (inbound_tx, inbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);
    let (outbound_tx, mut outbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);

    let reader_transport = Arc::clone(&transport);
    let mut reader_task = tokio::spawn(async move {
        loop {
            let Some(frame) = reader_transport.recv_frame().await? else {
                break Ok(());
            };
            if inbound_tx.send(frame).await.is_err() {
                break Ok(());
            }
        }
    });

    let writer_transport = Arc::clone(&transport);
    let outbound_messages_for_frames = outbound_messages.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            if let Some(outbound_messages) = &outbound_messages_for_frames {
                outbound_messages
                    .send(OutboundMessage::JsonRpcFrame(frame))
                    .await
                    .map_err(|_| TransportError::PeerDropped)?;
            } else {
                writer_transport.send_frame(frame).await?;
            }
        }
        Ok::<(), SdkAppServerBridgeError>(())
    });

    let owner = connection.run_frame_channels(inbound_rx, outbound_tx, handler);
    tokio::pin!(owner);
    let owner_result = tokio::select! {
        result = &mut owner => result.map_err(SdkAppServerBridgeError::from),
        reader = &mut reader_task => {
            match reader.map_err(SdkAppServerBridgeError::join)? {
                Ok(()) => owner.await.map_err(SdkAppServerBridgeError::from),
                Err(error) => {
                    let _ = owner.await;
                    Err(error)
                }
            }
        }
    };

    if !reader_task.is_finished() {
        reader_task.abort();
        let _ = reader_task.await;
    }
    writer_task.await.map_err(SdkAppServerBridgeError::join)??;
    owner_result
}

#[derive(Debug, thiserror::Error)]
pub enum SdkAppServerBridgeError {
    #[error("{source}")]
    Adapter { source: JsonRpcAdapterError },
    #[error("{source}")]
    Transport { source: TransportError },
    #[error("SDK app-server bridge task failed: {source}")]
    Join { source: tokio::task::JoinError },
}

impl SdkAppServerBridgeError {
    fn join(source: tokio::task::JoinError) -> Self {
        Self::Join { source }
    }
}

impl From<JsonRpcAdapterError> for SdkAppServerBridgeError {
    fn from(source: JsonRpcAdapterError) -> Self {
        Self::Adapter { source }
    }
}

impl From<TransportError> for SdkAppServerBridgeError {
    fn from(source: TransportError) -> Self {
        Self::Transport { source }
    }
}

#[cfg(test)]
#[path = "app_server_bridge.test.rs"]
mod tests;
