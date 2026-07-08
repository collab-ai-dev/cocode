//! Bridge from the new AppServer JSON-RPC adapter to the existing SDK handlers.
//!
//! This keeps runtime semantics in `coco-cli` while allowing
//! `coco-app-server` to own JSON-RPC connection dispatch and routing.

use std::collections::HashMap;
use std::sync::Arc;

use coco_app_server::AppCloseStart;
use coco_app_server::AppLoadStart;
use coco_app_server::AppServer;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::DisconnectOutcome;
use coco_app_server::JsonRpcAdapterConnection;
use coco_app_server::JsonRpcAdapterError;
use coco_app_server::JsonRpcDispatchError;
use coco_app_server::JsonRpcRequestContext;
use coco_app_server::JsonRpcRequestFuture;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientRequestContext;
use coco_app_server::LocalClientRequestFuture;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server_client::ClientError;
use coco_app_server_client::ServerClient;
use coco_app_server_client::SessionClient;
use coco_app_server_transport::JsonRpcErrorObject as TransportJsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse as TransportJsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcNotification as TransportJsonRpcNotification;
use coco_app_server_transport::JsonRpcRequest as TransportJsonRpcRequest;
use coco_app_server_transport::JsonRpcSuccess;
use coco_hub_connector::HubConnectorSender;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::JSONRPC_VERSION;
use coco_types::JsonRpcError;
use coco_types::JsonRpcErrorObject;
use coco_types::JsonRpcMessage;
use coco_types::JsonRpcNotification;
use coco_types::JsonRpcRequest;
use coco_types::JsonRpcResponse;
use coco_types::RequestId;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::warn;

use crate::sdk_server::dispatcher::SdkHubEgress;
use crate::sdk_server::dispatcher::spawn_sdk_outbound_writer;
use crate::sdk_server::handlers::HandlerContext;
use crate::sdk_server::handlers::HandlerResult;
use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::SessionHandle as SdkSessionHandle;
use crate::sdk_server::handlers::dispatch_client_request;
use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::transport::SdkTransport;
use crate::sdk_server::transport::TransportError;

const APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY: usize = 128;
const APP_SERVER_LOCAL_CHANNEL_CAPACITY: usize = 128;

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
}

/// Local AppServer registry handle for the current app/cli bridge.
///
/// The fused `SessionRuntime` can still be retargeted in-place by
/// `session/start` / `session/resume`, so this handle snapshots the registry
/// session id separately from the optional runtime handle. The close cascade is
/// intentionally retarget-safe until Phase B replaces in-place retargeting with
/// one immutable runtime handle per live session.
#[derive(Clone)]
pub struct LocalAppSessionHandle {
    session_id: SessionId,
    runtime: Option<crate::session_runtime::SessionHandle>,
}

impl LocalAppSessionHandle {
    fn snapshot(session_id: SessionId) -> Self {
        Self {
            session_id,
            runtime: None,
        }
    }

    fn from_runtime(session_id: SessionId, runtime: crate::session_runtime::SessionHandle) -> Self {
        Self {
            session_id,
            runtime: Some(runtime),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn runtime(&self) -> Option<&crate::session_runtime::SessionHandle> {
        self.runtime.as_ref()
    }
}

impl AppServerSdkHandler {
    pub fn new(state: Arc<SdkServerState>, notif_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: None,
        }
    }

    fn with_local_app_server(
        state: Arc<SdkServerState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<LocalAppSessionHandle>>,
    ) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: Some(app_server),
        }
    }
}

impl JsonRpcRequestHandler for AppServerSdkHandler {
    fn handle_json_rpc_request(
        &self,
        _context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
        };
        Box::pin(async move { dispatch_sdk_client_request(request, ctx).await })
    }
}

impl LocalClientRequestHandler for AppServerSdkHandler {
    fn handle_local_client_request(
        &self,
        _context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let local_app_server = self.local_app_server.clone();
        let lifecycle_request = local_app_server.as_ref().and_then(|app_server| {
            LocalLifecycleRequest::from_client_request(&request, app_server)
        });
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
        };
        Box::pin(async move {
            let result = dispatch_sdk_client_request(request, ctx).await?;
            if let (Some(app_server), Some(lifecycle_request)) =
                (local_app_server, lifecycle_request)
            {
                apply_local_lifecycle_request(app_server, lifecycle_request, &result).await?;
            }
            Ok(result)
        })
    }
}

#[derive(Debug, Clone)]
enum LocalLifecycleRequest {
    Start,
    Resume { live_before: Vec<SessionId> },
    Archive(SessionId),
}

impl LocalLifecycleRequest {
    fn from_client_request(
        request: &ClientRequest,
        app_server: &AppServer<LocalAppSessionHandle>,
    ) -> Option<Self> {
        match request {
            ClientRequest::SessionStart(_) => Some(Self::Start),
            ClientRequest::SessionResume(_) => Some(Self::Resume {
                live_before: app_server
                    .list_live_sessions()
                    .into_iter()
                    .map(|summary| summary.session_id)
                    .collect(),
            }),
            ClientRequest::SessionArchive(params) => Some(Self::Archive(params.session_id.clone())),
            _ => None,
        }
    }
}

async fn apply_local_lifecycle_request(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    request: LocalLifecycleRequest,
    result: &serde_json::Value,
) -> Result<(), JsonRpcDispatchError> {
    match request {
        LocalLifecycleRequest::Start => {
            let started: coco_types::SessionStartResult = serde_json::from_value(result.clone())
                .map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/start decode failed: {error}"),
                    data: None,
                })?;
            register_local_app_server_session(
                &app_server,
                LocalAppSessionHandle::snapshot(started.session_id),
            )
            .await?;
        }
        LocalLifecycleRequest::Resume { live_before } => {
            let resumed: coco_types::SessionResumeResult =
                serde_json::from_value(result.clone()).map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/resume decode failed: {error}"),
                    data: None,
                })?;
            let resumed_session_id = resumed.session.session_id;
            let mut replaced_existing = false;
            for previous_session_id in live_before {
                if previous_session_id != resumed_session_id {
                    if !replaced_existing
                        && replace_local_app_server_session(
                            Arc::clone(&app_server),
                            previous_session_id.clone(),
                            LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                        )
                        .await?
                    {
                        replaced_existing = true;
                    } else {
                        close_local_app_server_session(
                            Arc::clone(&app_server),
                            previous_session_id,
                        )
                        .await?;
                    }
                }
            }
            if !replaced_existing {
                register_local_app_server_session(
                    &app_server,
                    LocalAppSessionHandle::snapshot(resumed_session_id),
                )
                .await?;
            }
        }
        LocalLifecycleRequest::Archive(session_id) => {
            close_local_app_server_session(app_server, session_id).await?;
        }
    }
    Ok(())
}

async fn replace_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    old_session_id: SessionId,
    new_handle: LocalAppSessionHandle,
) -> Result<bool, JsonRpcDispatchError> {
    let Some(calling_surface) = local_replace_calling_surface(&app_server, &old_session_id) else {
        return Ok(false);
    };
    let new_session_id = new_handle.session_id.clone();
    let mut completion = match app_server
        .spawn_replace(
            old_session_id,
            new_session_id,
            calling_surface,
            async { Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(new_handle) },
            close_local_session_handle,
        )
        .map_err(|error| local_lifecycle_error("replace session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map(|_| true)
        .map_err(|error| local_lifecycle_error("replace session", error))
}

fn local_replace_calling_surface(
    app_server: &AppServer<LocalAppSessionHandle>,
    session_id: &SessionId,
) -> Option<SurfaceId> {
    let routing = app_server
        .routing()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    routing.interactive_owner(session_id).cloned()
}

async fn register_local_app_server_session(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    handle: LocalAppSessionHandle,
) -> Result<(), JsonRpcDispatchError> {
    let session_id = handle.session_id.clone();
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
            if handle.runtime().is_some() {
                let refresh_session_id = handle.session_id.clone();
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

async fn close_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
) -> Result<(), JsonRpcDispatchError> {
    if !app_server
        .list_live_sessions()
        .iter()
        .any(|summary| summary.session_id == session_id)
    {
        return Ok(());
    }
    let mut completion = match app_server
        .spawn_close(session_id, close_local_session_handle)
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

async fn close_local_session_handle(handle: LocalAppSessionHandle) {
    let has_runtime = handle.runtime.is_some();
    if let Some(runtime) = handle.runtime() {
        let current = runtime.current_typed_session_id().await;
        if current != *handle.session_id() {
            debug!(
                target: "coco::app_server_local",
                registry_session_id = %handle.session_id(),
                current_session_id = %current,
                "skipping local AppServer close cascade for retargeted fused runtime"
            );
            return;
        }
    }
    debug!(
        target: "coco::app_server_local",
        session_id = %handle.session_id(),
        has_runtime,
        "local AppServer close cascade reached fused runtime boundary"
    );
}

fn local_lifecycle_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("local AppServer {operation} failed: {error}"),
        data: None,
    }
}

pub struct AppServerLocalBridge {
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    client: ServerClient<LocalAppSessionHandle>,
    handler: AppServerSdkHandler,
    outbound_forwarder: JoinHandle<()>,
    event_pump: Option<JoinHandle<()>>,
    event_pump_session_id: Option<SessionId>,
    interactive_surface: Option<SessionClient>,
    channel_capacity: usize,
}

impl AppServerLocalBridge {
    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self::with_channel_capacity(state, APP_SERVER_LOCAL_CHANNEL_CAPACITY)
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
        Self::with_channel_capacity_and_hub_connector(state, channel_capacity, None)
    }

    fn with_channel_capacity_and_hub_connector(
        state: Arc<SdkServerState>,
        channel_capacity: usize,
        hub_connector: Option<HubConnectorSender>,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "local AppServer bridge channel capacity must be non-zero"
        );
        let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(
            /*max_sessions*/ 1,
            channel_capacity,
        ));
        let adapter =
            LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), channel_capacity);
        let client = ServerClient::connect_local(&adapter);
        let (outbound_tx, outbound_rx) = mpsc::channel(channel_capacity);
        let handler = AppServerSdkHandler::with_local_app_server(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
        );
        let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
            Arc::clone(&app_server),
            state,
            outbound_rx,
            hub_connector,
        );
        Self {
            app_server,
            client,
            handler,
            outbound_forwarder,
            event_pump: None,
            event_pump_session_id: None,
            interactive_surface: None,
            channel_capacity,
        }
    }

    pub fn app_server(&self) -> &Arc<AppServer<LocalAppSessionHandle>> {
        &self.app_server
    }

    pub fn client(&self) -> &ServerClient<LocalAppSessionHandle> {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut ServerClient<LocalAppSessionHandle> {
        &mut self.client
    }

    pub fn connect_local_client(&self) -> ServerClient<LocalAppSessionHandle> {
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        ServerClient::connect_local(&adapter)
    }

    pub fn handler(&self) -> &AppServerSdkHandler {
        &self.handler
    }

    pub async fn close_registered_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        close_local_app_server_session(Arc::clone(&self.app_server), session_id)
            .await
            .map_err(|error| ClientError::Server {
                code: error.code,
                message: error.message,
                data: error.data,
            })
    }

    pub fn ensure_interactive_surface(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        if self
            .interactive_surface
            .as_ref()
            .is_some_and(|surface| surface.session_id() == &session_id)
        {
            return Ok(());
        }
        if let Some(surface) = self.interactive_surface.as_ref()
            && self.surface_is_attached_to_session(surface.surface_id(), &session_id)
        {
            self.interactive_surface = Some(surface.with_session_id(session_id));
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
        let routing = self
            .app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        routing.surface_session(surface_id) == Some(session_id)
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
            .map_err(ClientError::from)?;
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
        params: coco_types::TurnStartParams,
    ) -> Result<AppServerLocalTurnCompletion, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
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
        params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        self.client.turn_start(&self.handler, params).await
    }

    pub async fn current_session_result(&self) -> Option<coco_types::SessionResultParams> {
        let slot = self.handler.state.session.read().await;
        let session = slot.as_ref()?;
        let stats = &session.stats;
        Some(coco_types::SessionResultParams {
            session_id: session.session_id.clone(),
            total_turns: stats.total_turns,
            duration_ms: session.started_at.elapsed().as_millis() as i64,
            duration_api_ms: stats.total_duration_api_ms,
            is_error: stats.had_error,
            stop_reason: stats
                .last_stop_reason
                .clone()
                .unwrap_or_else(|| "end_turn".into()),
            total_cost_usd: stats.total_cost_usd,
            usage: stats.usage,
            model_usage: stats.model_usage.clone(),
            permission_denials: stats.permission_denials.clone(),
            result: stats.last_result_text.clone(),
            errors: stats.errors.clone(),
            structured_output: if stats.had_error {
                None
            } else {
                stats.structured_output.clone()
            },
            fast_mode_state: None,
            num_api_calls: (stats.num_api_calls > 0).then_some(stats.num_api_calls),
        })
    }

    pub async fn install_session_snapshot(
        &self,
        session_id: SessionId,
        cwd: impl Into<String>,
        model: impl Into<String>,
    ) {
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            LocalAppSessionHandle::snapshot(session_id.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry snapshot install failed");
        }
        self.install_sdk_session_handle(SdkSessionHandle::new(
            session_id,
            cwd.into(),
            model.into(),
        ))
        .await;
    }

    pub async fn install_session_runtime(&self, session: crate::session_runtime::SessionHandle) {
        crate::sdk_server::sdk_hooks::install_runtime_callback(
            Arc::clone(&self.handler.state),
            &session,
        );
        let (
            session_id,
            cwd,
            model,
            max_turns,
            system_prompt,
            permission_mode,
            thinking_level,
            bypass_permissions_available,
            app_state,
            history,
            session_manager,
            file_history,
            config_home,
        ) = {
            let runtime = session.runtime();
            let session_id = session.session_id().clone();
            let cwd = runtime
                .current_cwd
                .read()
                .await
                .to_string_lossy()
                .into_owned();
            let config = runtime.current_engine_config().await;
            let history = runtime.history.lock().await.iter().cloned().collect();
            (
                session_id,
                cwd,
                config.model_id.clone(),
                config.max_turns,
                config.system_prompt.clone(),
                config.permission_mode,
                config.thinking_level.clone(),
                config.permission_mode_availability.bypass_permissions,
                Arc::clone(&runtime.app_state),
                history,
                Arc::clone(&runtime.session_manager),
                runtime.file_history.clone(),
                runtime.config_home.clone(),
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

        let mut sdk_session = SdkSessionHandle::new(session_id, cwd, model);
        sdk_session.permission_mode = Some(permission_mode);
        sdk_session.thinking_level = thinking_level;
        sdk_session.app_state = app_state;
        sdk_session.history = Arc::new(tokio::sync::Mutex::new(history));
        self.install_sdk_session_handle(sdk_session).await;
        self.handler.state.bypass_permissions_available.store(
            bypass_permissions_available,
            std::sync::atomic::Ordering::Relaxed,
        );
        {
            let mut runner = self.handler.state.turn_runner.write().await;
            *runner = Arc::new(crate::sdk_server::QueryEngineRunner::new(
                session.clone(),
                max_turns,
                system_prompt,
            ));
        }
        {
            let mut slot = self.handler.state.session_manager.write().await;
            *slot = Some(session_manager);
        }
        {
            let mut slot = self.handler.state.file_history.write().await;
            *slot = file_history;
        }
        {
            let mut slot = self.handler.state.file_history_config_home.write().await;
            *slot = Some(config_home);
        }
        let mut slot = self.handler.state.session_runtime.write().await;
        *slot = Some(session);
    }

    async fn install_sdk_session_handle(&self, session: SdkSessionHandle) {
        let mut slot = self.handler.state.session.write().await;
        *slot = Some(session);
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
    hub_connector: Option<HubConnectorSender>,
) -> JoinHandle<()>
where
    H: Clone + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut next_session_seq: HashMap<SessionId, i64> = HashMap::new();
        while let Some(outbound) = outbound_rx.recv().await {
            match outbound {
                OutboundMessage::CoreEvent(event) => {
                    let Some(session_id) = current_app_server_session_id(&state).await else {
                        warn!("dropping local AppServer event without an active session");
                        continue;
                    };
                    let seq_session_id = session_id.clone();
                    let envelope = SessionEnvelope::stamp(session_id, None, *event, || {
                        let next = next_session_seq.entry(seq_session_id).or_insert(1);
                        let seq = *next;
                        *next += 1;
                        seq
                    });
                    let hub_envelope = envelope.clone();
                    server.route_envelope(envelope);
                    if let Some(hub_connector) = &hub_connector
                        && let Err(error) = hub_connector.try_enqueue(hub_envelope)
                    {
                        warn!(%error, "dropping local AppServer event from Hub connector queue");
                    }
                }
                OutboundMessage::JsonRpc(_) => {
                    warn!("dropping JSON-RPC outbound message on local AppServer forwarder");
                }
            }
        }
    })
}

async fn current_app_server_session_id(state: &SdkServerState) -> Option<SessionId> {
    if let Some(session) = state.session.read().await.as_ref() {
        return Some(session.session_id.clone());
    }
    let runtime = state.session_runtime.read().await.clone();
    if let Some(runtime) = runtime {
        return Some(runtime.current_typed_session_id().await);
    }
    None
}

#[cfg(test)]
fn decode_client_request(
    method: impl Into<String>,
    params: Option<serde_json::Value>,
) -> Result<ClientRequest, serde_json::Error> {
    let method = method.into();
    let mut object = serde_json::Map::new();
    object.insert(
        "method".to_string(),
        serde_json::Value::String(method.clone()),
    );
    if let Some(params) = params {
        object.insert("params".to_string(), params);
    }

    let with_params = serde_json::Value::Object(object);
    match serde_json::from_value(with_params) {
        Ok(request) => Ok(request),
        Err(with_params_error) => {
            let without_params = serde_json::json!({ "method": method });
            serde_json::from_value(without_params).map_err(|_| with_params_error)
        }
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

#[cfg(test)]
#[allow(dead_code)]
async fn run_app_server_connection_over_sdk_transport<H, Handler>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    handler: Arc<Handler>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
    Handler: JsonRpcRequestHandler,
{
    run_app_server_connection_over_sdk_transport_inner(connection, transport, handler, None, None)
        .await
}

#[cfg(test)]
async fn run_app_server_sdk_state_over_sdk_transport<H>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
{
    run_app_server_sdk_state_over_sdk_transport_with_external_notifications(
        connection,
        transport,
        state,
        Vec::new(),
    )
    .await
}

#[cfg(test)]
async fn run_app_server_sdk_state_over_sdk_transport_with_external_notifications<H>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
{
    run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
        connection,
        transport,
        state,
        external_notifications,
        None,
    )
    .await
}

pub async fn run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector<
    H,
>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
    hub_connector: Option<HubConnectorSender>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
{
    let (outbound_tx, outbound_rx) = mpsc::channel::<OutboundMessage>(256);
    {
        let mut slot = state.transport.write().await;
        *slot = Some(Arc::clone(&transport));
    }
    {
        let mut slot = state.outbound_tx.write().await;
        *slot = Some(outbound_tx.clone());
    }

    let mcp_manager = {
        let slot = state.mcp_manager.read().await;
        slot.as_ref().cloned()
    };
    if let Some(manager) = mcp_manager {
        crate::sdk_server::sdk_mcp::install_route(
            manager,
            Arc::clone(&state),
            Arc::clone(&transport),
        )
        .await;
    }

    let mut external_forwarders = Vec::new();
    for mut rx in external_notifications {
        let forwarded_tx = outbound_tx.clone();
        external_forwarders.push(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if forwarded_tx
                    .send(OutboundMessage::core_event(event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    let hub_egress = hub_connector.map(|sender| SdkHubEgress::new(Arc::clone(&state), sender));
    let writer_task = spawn_sdk_outbound_writer(Arc::clone(&transport), outbound_rx, hub_egress);
    let handler = Arc::new(AppServerSdkHandler::new(
        Arc::clone(&state),
        outbound_tx.clone(),
    ));
    let result = run_app_server_connection_over_sdk_transport_inner(
        connection,
        transport,
        handler,
        Some(outbound_tx.clone()),
        Some(Arc::clone(&state)),
    )
    .await;

    {
        let mut slot = state.outbound_tx.write().await;
        *slot = None;
    }
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
    legacy_response_state: Option<Arc<SdkServerState>>,
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
            let Some(message) = reader_transport.recv().await? else {
                break Ok(());
            };
            if matches!(
                message,
                JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_)
            ) && let Some(state) = &legacy_response_state
                && state.resolve_server_request(message.clone()).await
            {
                continue;
            }
            let frame = legacy_json_rpc_message_to_frame(message)?;
            if inbound_tx.send(frame).await.is_err() {
                break Ok(());
            }
        }
    });

    let writer_transport = Arc::clone(&transport);
    let outbound_messages_for_frames = outbound_messages.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            let message = json_rpc_frame_to_legacy_message(frame)?;
            if let Some(outbound_messages) = &outbound_messages_for_frames {
                outbound_messages
                    .send(OutboundMessage::JsonRpc(message))
                    .await
                    .map_err(|_| TransportError::PeerDropped)?;
            } else {
                writer_transport.send(message).await?;
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

fn legacy_json_rpc_message_to_frame(
    message: JsonRpcMessage,
) -> Result<JsonRpcFrame, JsonRpcBridgeError> {
    match message {
        JsonRpcMessage::Request(request) => {
            Ok(JsonRpcFrame::Request(TransportJsonRpcRequest::new(
                json_rpc_id_from_request_id(request.request_id),
                request.method,
                Some(request.params),
            )))
        }
        JsonRpcMessage::Response(response) => Ok(JsonRpcFrame::Success(JsonRpcSuccess::new(
            json_rpc_id_from_request_id(response.request_id),
            response.result,
        ))),
        JsonRpcMessage::Error(error) => {
            Ok(JsonRpcFrame::Error(TransportJsonRpcErrorResponse::new(
                json_rpc_id_from_request_id(error.request_id),
                TransportJsonRpcErrorObject::new(
                    error.error.code,
                    error.error.message,
                    error.error.data,
                ),
            )))
        }
        JsonRpcMessage::Notification(notification) => Ok(JsonRpcFrame::Notification(
            TransportJsonRpcNotification::new(notification.method, Some(notification.params)),
        )),
    }
}

fn json_rpc_frame_to_legacy_message(
    frame: JsonRpcFrame,
) -> Result<JsonRpcMessage, JsonRpcBridgeError> {
    match frame {
        JsonRpcFrame::Request(request) => Ok(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(request.id)?,
            method: request.method,
            params: request.params.unwrap_or(serde_json::Value::Null),
        })),
        JsonRpcFrame::Success(success) => Ok(JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(success.id)?,
            result: success.result,
        })),
        JsonRpcFrame::Error(error) => Ok(JsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(error.id)?,
            error: JsonRpcErrorObject {
                code: error.error.code,
                message: error.error.message,
                data: error.error.data,
            },
        })),
        JsonRpcFrame::Notification(notification) => {
            Ok(JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.into(),
                method: notification.method,
                params: notification.params.unwrap_or(serde_json::Value::Null),
            }))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JsonRpcBridgeError {
    #[error("legacy SDK JSON-RPC ids cannot represent null")]
    NullId,
}

#[derive(Debug, thiserror::Error)]
pub enum SdkAppServerBridgeError {
    #[error("{source}")]
    Adapter { source: JsonRpcAdapterError },
    #[error("{source}")]
    Transport { source: TransportError },
    #[error("{source}")]
    JsonRpcBridge { source: JsonRpcBridgeError },
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

impl From<JsonRpcBridgeError> for SdkAppServerBridgeError {
    fn from(source: JsonRpcBridgeError) -> Self {
        Self::JsonRpcBridge { source }
    }
}

fn json_rpc_id_from_request_id(request_id: RequestId) -> JsonRpcId {
    match request_id {
        RequestId::Integer(value) => JsonRpcId::Number(value),
        RequestId::String(value) => JsonRpcId::String(value),
    }
}

fn request_id_from_json_rpc_id(id: JsonRpcId) -> Result<RequestId, JsonRpcBridgeError> {
    match id {
        JsonRpcId::Number(value) => Ok(RequestId::Integer(value)),
        JsonRpcId::String(value) => Ok(RequestId::String(value)),
        JsonRpcId::Null => Err(JsonRpcBridgeError::NullId),
    }
}

#[cfg(test)]
#[path = "app_server_bridge.test.rs"]
mod tests;
