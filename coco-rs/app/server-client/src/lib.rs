//! Remote typed AppServer client.
//!
//! Owns the transport-agnostic JSON-RPC core, per-surface event demultiplexing,
//! and concrete remote dialing over caller-owned NDJSON streams, Unix domain
//! sockets, Windows named pipes, and WebSockets. The in-process client facade
//! lives in `coco-agent-host::local_client`, keeping this crate independent of
//! the server implementation.

mod remote_demux;
mod remote_transport;

pub use remote_demux::RemoteEventDemux;
pub use remote_demux::RemoteJsonRpcEvent;
pub use remote_demux::RemoteOwnedSurfaceStream;
pub use remote_demux::RemoteSurfaceStream;
pub use remote_transport::RemoteDefaultWebSocketConnection;
pub use remote_transport::RemoteNdjsonConnection;
#[cfg(windows)]
pub use remote_transport::RemoteNdjsonNamedPipeConnection;
#[cfg(unix)]
pub use remote_transport::RemoteNdjsonUnixConnection;
pub use remote_transport::RemoteWebSocketConnection;

use remote_demux::decode_session_subscribe_envelope;
use remote_demux::remote_event_from_notification;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_app_server_transport::JsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcRequest;
use coco_app_server_transport::JsonRpcSuccess;
use coco_app_server_transport::NdjsonDuplexConnection;
use coco_app_server_transport::TransportFrameError;
use coco_types::AgentInterruptCurrentWorkParams;
use coco_types::ApplyPermissionUpdateParams;
use coco_types::ApprovalResolveParams;
use coco_types::BackgroundAllTasksResult;
use coco_types::CancelRequestParams;
use coco_types::ClientRequest;
use coco_types::ConfigApplyFlagsParams;
use coco_types::ConfigReadResult;
use coco_types::ConfigWriteParams;
use coco_types::ContextUsageResult;
use coco_types::ElicitationResolveParams;
use coco_types::HookReloadResult;
use coco_types::InitializeParams;
use coco_types::InitializeResult;
use coco_types::McpReconnectParams;
use coco_types::McpSetServersParams;
use coco_types::McpSetServersResult;
use coco_types::McpStatusResult;
use coco_types::McpToggleParams;
use coco_types::PluginReloadResult;
use coco_types::ResetSessionPermissionRulesResult;
use coco_types::RewindFilesParams;
use coco_types::RewindFilesResult;
use coco_types::SessionArchiveParams;
use coco_types::SessionCostResult;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionListResult;
use coco_types::SessionReadParams;
use coco_types::SessionReadResult;
use coco_types::SessionRenameParams;
use coco_types::SessionRenameResult;
use coco_types::SessionResumeParams;
use coco_types::SessionResumeResult;
use coco_types::SessionStartParams;
use coco_types::SessionStartResult;
use coco_types::SessionStatusResult;
use coco_types::SessionSubscribeParams;
use coco_types::SessionSubscribeResult;
use coco_types::SessionToggleTagParams;
use coco_types::SessionToggleTagResult;
use coco_types::SessionTurnsListParams;
use coco_types::SessionTurnsListResult;
use coco_types::SetAgentColorParams;
use coco_types::SetModelParams;
use coco_types::SetModelRoleParams;
use coco_types::SetModelRoleResult;
use coco_types::SetPermissionModeParams;
use coco_types::SetThinkingParams;
use coco_types::StopTaskParams;
use coco_types::SurfaceId;
use coco_types::SurfaceLifecycleEffect;
use coco_types::TaskDetailParams;
use coco_types::TaskDetailResult;
use coco_types::TaskListResult;
use coco_types::TurnStartParams;
use coco_types::TurnStartResult;
use coco_types::UpdateEnvParams;
use coco_types::UserInputResolveParams;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_tungstenite::connect_async;

const DEFAULT_REMOTE_EVENT_CHANNEL_CAPACITY: usize = 128;
const DEFAULT_REMOTE_OUTBOUND_CHANNEL_CAPACITY: usize = 128;
/// Client-side outbound write bound mirroring the server's slow-consumer guard.
/// A stalled write would otherwise freeze inbound processing on that connection.
const DEFAULT_REMOTE_WRITE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(30));
/// Cap on the demux's connection-scoped (non-surface-keyed) buffers so a peer
/// that floods notifications / server requests without a reader cannot grow the
/// client unboundedly. Drop-oldest with a warning once full.
const MAX_BUFFERED_CONNECTION_QUEUE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteConnectOptions {
    pub outbound_channel_capacity: usize,
    pub event_channel_capacity: usize,
    /// Per-request timeout for remote JSON-RPC requests. `None` (the default)
    /// waits indefinitely for a response.
    pub request_timeout: Option<Duration>,
    /// Per-frame outbound write timeout. On expiry the owner loop fails with
    /// `RemoteTransportError::SlowConsumer` and disconnects. `None` disables it.
    pub write_timeout: Option<Duration>,
}

impl Default for RemoteConnectOptions {
    fn default() -> Self {
        Self {
            outbound_channel_capacity: DEFAULT_REMOTE_OUTBOUND_CHANNEL_CAPACITY,
            event_channel_capacity: DEFAULT_REMOTE_EVENT_CHANNEL_CAPACITY,
            request_timeout: None,
            write_timeout: DEFAULT_REMOTE_WRITE_TIMEOUT,
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<JsonRpcId, PendingRemoteRequest>>>;

#[derive(Clone)]
pub struct RemoteJsonRpcClient {
    outbound: mpsc::Sender<JsonRpcFrame>,
    pending: PendingMap,
    invalid: Arc<AtomicBool>,
    next_request_id: Arc<AtomicI64>,
    request_timeout: Option<Duration>,
}

pub struct RemoteJsonRpcIncoming {
    pending: PendingMap,
    events: mpsc::Sender<RemoteJsonRpcEvent>,
    invalid: Arc<AtomicBool>,
}

/// Not `Clone`: `replace_*`/`close` consume the handle, so consume-self is
/// type-enforced and a still-live session cannot be silently orphaned.
pub struct RemoteSessionClient {
    client: RemoteJsonRpcClient,
    session_id: SessionId,
    surface_id: SurfaceId,
}

/// Not `Clone`: see `RemoteSessionClient`.
pub struct RemotePassiveSessionClient {
    client: RemoteJsonRpcClient,
    session_id: SessionId,
    surface_id: SurfaceId,
    replayed: Vec<SessionEnvelope>,
}

struct PendingRemoteRequest {
    reply: oneshot::Sender<Result<serde_json::Value, ClientError>>,
}

/// Lock the pending-response map, tolerating poison. Every critical section is
/// non-await (insert / remove / drain), so a `std::sync::Mutex` is sound and
/// lets `Drop` resolve pending futures without an async context.
fn lock_pending(pending: &Mutex<HashMap<JsonRpcId, PendingRemoteRequest>>) -> PendingGuard<'_> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

type PendingGuard<'a> = MutexGuard<'a, HashMap<JsonRpcId, PendingRemoteRequest>>;

impl RemoteJsonRpcClient {
    pub fn new(
        outbound: mpsc::Sender<JsonRpcFrame>,
    ) -> (
        Self,
        RemoteJsonRpcIncoming,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        Self::with_event_channel_capacity(outbound, DEFAULT_REMOTE_EVENT_CHANNEL_CAPACITY)
    }

    pub fn with_event_channel_capacity(
        outbound: mpsc::Sender<JsonRpcFrame>,
        event_channel_capacity: usize,
    ) -> (
        Self,
        RemoteJsonRpcIncoming,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        Self::with_event_channel_capacity_and_timeout(
            outbound,
            event_channel_capacity,
            /*request_timeout*/ None,
        )
    }

    fn with_event_channel_capacity_and_timeout(
        outbound: mpsc::Sender<JsonRpcFrame>,
        event_channel_capacity: usize,
        request_timeout: Option<Duration>,
    ) -> (
        Self,
        RemoteJsonRpcIncoming,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        assert!(
            event_channel_capacity > 0,
            "remote event channel capacity must be non-zero"
        );
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let invalid = Arc::new(AtomicBool::new(false));
        let (events_tx, events_rx) = mpsc::channel(event_channel_capacity);
        let client = Self {
            outbound,
            pending: Arc::clone(&pending),
            invalid: Arc::clone(&invalid),
            next_request_id: Arc::new(AtomicI64::new(1)),
            request_timeout,
        };
        let incoming = RemoteJsonRpcIncoming {
            pending,
            events: events_tx,
            invalid,
        };
        (client, incoming, events_rx)
    }

    pub fn connect_ndjson<R, W>(
        transport: NdjsonDuplexConnection<R, W>,
    ) -> (
        Self,
        RemoteNdjsonConnection<R, W>,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        Self::connect_ndjson_with_options(transport, RemoteConnectOptions::default())
    }

    pub fn connect_ndjson_with_options<R, W>(
        transport: NdjsonDuplexConnection<R, W>,
        options: RemoteConnectOptions,
    ) -> (
        Self,
        RemoteNdjsonConnection<R, W>,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        assert!(
            options.outbound_channel_capacity > 0,
            "remote outbound channel capacity must be non-zero"
        );
        let (outbound_tx, outbound_rx) = mpsc::channel(options.outbound_channel_capacity);
        let (client, incoming, events) = Self::with_event_channel_capacity_and_timeout(
            outbound_tx,
            options.event_channel_capacity,
            options.request_timeout,
        );
        let connection = RemoteNdjsonConnection {
            incoming,
            outbound: outbound_rx,
            transport,
            write_timeout: options.write_timeout,
        };
        (client, connection, events)
    }

    pub fn connect_ndjson_with_channel_capacity<R, W>(
        transport: NdjsonDuplexConnection<R, W>,
        outbound_channel_capacity: usize,
        event_channel_capacity: usize,
    ) -> (
        Self,
        RemoteNdjsonConnection<R, W>,
        mpsc::Receiver<RemoteJsonRpcEvent>,
    ) {
        Self::connect_ndjson_with_options(
            transport,
            RemoteConnectOptions {
                outbound_channel_capacity,
                event_channel_capacity,
                request_timeout: None,
                write_timeout: DEFAULT_REMOTE_WRITE_TIMEOUT,
            },
        )
    }

    #[cfg(unix)]
    pub async fn connect_unix(
        path: impl AsRef<std::path::Path>,
    ) -> Result<
        (
            Self,
            RemoteNdjsonUnixConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_unix_with_options(path, RemoteConnectOptions::default()).await
    }

    #[cfg(unix)]
    pub async fn connect_unix_with_options(
        path: impl AsRef<std::path::Path>,
        options: RemoteConnectOptions,
    ) -> Result<
        (
            Self,
            RemoteNdjsonUnixConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        let transport = coco_app_server_transport::connect_ndjson_unix(path)
            .await
            .map_err(|error| ClientError::Connect(error.to_string()))?;
        Ok(Self::connect_ndjson_with_options(transport, options))
    }

    #[cfg(unix)]
    pub async fn connect_unix_with_channel_capacity(
        path: impl AsRef<std::path::Path>,
        outbound_channel_capacity: usize,
        event_channel_capacity: usize,
    ) -> Result<
        (
            Self,
            RemoteNdjsonUnixConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_unix_with_options(
            path,
            RemoteConnectOptions {
                outbound_channel_capacity,
                event_channel_capacity,
                request_timeout: None,
                write_timeout: DEFAULT_REMOTE_WRITE_TIMEOUT,
            },
        )
        .await
    }

    #[cfg(windows)]
    pub async fn connect_named_pipe(
        pipe_name: impl AsRef<str>,
    ) -> Result<
        (
            Self,
            RemoteNdjsonNamedPipeConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_named_pipe_with_options(pipe_name, RemoteConnectOptions::default()).await
    }

    #[cfg(windows)]
    pub async fn connect_named_pipe_with_options(
        pipe_name: impl AsRef<str>,
        options: RemoteConnectOptions,
    ) -> Result<
        (
            Self,
            RemoteNdjsonNamedPipeConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        let transport = coco_app_server_transport::connect_ndjson_named_pipe(pipe_name)
            .map_err(|error| ClientError::Connect(error.to_string()))?;
        Ok(Self::connect_ndjson_with_options(transport, options))
    }

    #[cfg(windows)]
    pub async fn connect_named_pipe_with_channel_capacity(
        pipe_name: impl AsRef<str>,
        outbound_channel_capacity: usize,
        event_channel_capacity: usize,
    ) -> Result<
        (
            Self,
            RemoteNdjsonNamedPipeConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_named_pipe_with_options(
            pipe_name,
            RemoteConnectOptions {
                outbound_channel_capacity,
                event_channel_capacity,
                request_timeout: None,
                write_timeout: DEFAULT_REMOTE_WRITE_TIMEOUT,
            },
        )
        .await
    }

    pub async fn connect_websocket(
        url: &str,
    ) -> Result<
        (
            Self,
            RemoteDefaultWebSocketConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_websocket_with_options(url, RemoteConnectOptions::default()).await
    }

    pub async fn connect_websocket_with_options(
        url: &str,
        options: RemoteConnectOptions,
    ) -> Result<
        (
            Self,
            RemoteDefaultWebSocketConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        assert!(
            options.outbound_channel_capacity > 0,
            "remote outbound channel capacity must be non-zero"
        );
        let (websocket, _) = connect_async(url)
            .await
            .map_err(|error| ClientError::Connect(error.to_string()))?;
        let (outbound_tx, outbound_rx) = mpsc::channel(options.outbound_channel_capacity);
        let (client, incoming, events) = Self::with_event_channel_capacity_and_timeout(
            outbound_tx,
            options.event_channel_capacity,
            options.request_timeout,
        );
        let connection = RemoteWebSocketConnection {
            incoming,
            outbound: outbound_rx,
            websocket,
            write_timeout: options.write_timeout,
        };
        Ok((client, connection, events))
    }

    pub async fn connect_websocket_with_channel_capacity(
        url: &str,
        outbound_channel_capacity: usize,
        event_channel_capacity: usize,
    ) -> Result<
        (
            Self,
            RemoteDefaultWebSocketConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        ClientError,
    > {
        Self::connect_websocket_with_options(
            url,
            RemoteConnectOptions {
                outbound_channel_capacity,
                event_channel_capacity,
                request_timeout: None,
                write_timeout: DEFAULT_REMOTE_WRITE_TIMEOUT,
            },
        )
        .await
    }

    pub async fn send_client_request(
        &self,
        request: ClientRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let (method, params) = client_request_method_and_params(&request)?;
        self.request(method, params).await
    }

    pub fn session_handle(
        &self,
        session_id: SessionId,
        surface_id: SurfaceId,
    ) -> RemoteSessionClient {
        RemoteSessionClient {
            client: self.clone(),
            session_id,
            surface_id,
        }
    }

    pub fn passive_session_handle(
        &self,
        session_id: SessionId,
        surface_id: SurfaceId,
    ) -> RemotePassiveSessionClient {
        RemotePassiveSessionClient {
            client: self.clone(),
            session_id,
            surface_id,
            replayed: Vec::new(),
        }
    }

    pub async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, ClientError> {
        self.send_typed_client_request(ClientRequest::Initialize(params))
            .await
    }

    pub async fn keep_alive(&self) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::KeepAlive)
            .await
    }

    pub async fn session_start(
        &self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionStart(Box::new(params)))
            .await
    }

    pub async fn session_start_handle(
        &self,
        demux: &mut RemoteEventDemux,
        params: SessionStartParams,
    ) -> Result<RemoteSessionClient, ClientError> {
        let started = self.session_start(params).await?;
        let surface_id = match started.surface_id {
            Some(surface_id) => surface_id,
            None => {
                demux
                    .next_session_activation(&started.session_id)
                    .await
                    .ok_or(ClientError::Disconnected)?
                    .surface_id
            }
        };
        Ok(self.session_handle(started.session_id, surface_id))
    }

    pub async fn session_resume(
        &self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionResume(params))
            .await
    }

    pub async fn session_resume_handle(
        &self,
        demux: &mut RemoteEventDemux,
        params: SessionResumeParams,
    ) -> Result<RemoteSessionClient, ClientError> {
        let resumed = self.session_resume(params).await?;
        let session_id = resumed.session.session_id;
        let surface_id = match resumed.surface_id {
            Some(surface_id) => surface_id,
            None => {
                demux
                    .next_session_activation(&session_id)
                    .await
                    .ok_or(ClientError::Disconnected)?
                    .surface_id
            }
        };
        Ok(self.session_handle(session_id, surface_id))
    }

    pub async fn session_list(&self) -> Result<SessionListResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionList)
            .await
    }

    pub async fn session_read(
        &self,
        params: SessionReadParams,
    ) -> Result<SessionReadResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionRead(params))
            .await
    }

    pub async fn session_turns_list(
        &self,
        params: SessionTurnsListParams,
    ) -> Result<SessionTurnsListResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionTurnsList(params))
            .await
    }

    pub async fn session_subscribe(
        &self,
        params: SessionSubscribeParams,
    ) -> Result<SessionSubscribeResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionSubscribe(params))
            .await
    }

    pub async fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
    ) -> Result<RemotePassiveSessionClient, ClientError> {
        let subscribed = self
            .session_subscribe(SessionSubscribeParams {
                session_id,
                after_seq,
            })
            .await?;
        let replayed = subscribed
            .replayed
            .into_iter()
            .map(decode_session_subscribe_envelope)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RemotePassiveSessionClient {
            client: self.clone(),
            session_id: subscribed.session_id,
            surface_id: subscribed.surface_id,
            replayed,
        })
    }

    pub async fn session_archive(&self, params: SessionArchiveParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SessionArchive(params))
            .await
    }

    pub async fn session_rename(
        &self,
        params: SessionRenameParams,
    ) -> Result<SessionRenameResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionRename(params))
            .await
    }

    pub async fn session_toggle_tag(
        &self,
        params: SessionToggleTagParams,
    ) -> Result<SessionToggleTagResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionToggleTag(params))
            .await
    }

    pub async fn session_cost(&self) -> Result<SessionCostResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionCost)
            .await
    }

    pub async fn session_status(&self) -> Result<SessionStatusResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionStatus)
            .await
    }

    pub async fn turn_start(
        &self,
        params: TurnStartParams,
    ) -> Result<TurnStartResult, ClientError> {
        self.send_typed_client_request(ClientRequest::TurnStart(params))
            .await
    }

    pub async fn turn_interrupt(&self) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::TurnInterrupt)
            .await
    }

    pub async fn approval_resolve(&self, params: ApprovalResolveParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::ApprovalResolve(params))
            .await
    }

    pub async fn user_input_resolve(
        &self,
        params: UserInputResolveParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::UserInputResolve(params))
            .await
    }

    pub async fn elicitation_resolve(
        &self,
        params: ElicitationResolveParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::ElicitationResolve(params))
            .await
    }

    pub async fn set_model(&self, params: SetModelParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SetModel(params))
            .await
    }

    pub async fn set_model_role(
        &self,
        params: SetModelRoleParams,
    ) -> Result<SetModelRoleResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SetModelRole(params))
            .await
    }

    pub async fn set_permission_mode(
        &self,
        params: SetPermissionModeParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SetPermissionMode(params))
            .await
    }

    pub async fn set_thinking(&self, params: SetThinkingParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SetThinking(params))
            .await
    }

    pub async fn set_agent_color(&self, params: SetAgentColorParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SetAgentColor(params))
            .await
    }

    pub async fn apply_permission_update(
        &self,
        params: ApplyPermissionUpdateParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::ApplyPermissionUpdate(params))
            .await
    }

    pub async fn reset_session_permission_rules(
        &self,
    ) -> Result<ResetSessionPermissionRulesResult, ClientError> {
        self.send_typed_client_request(ClientRequest::ResetSessionPermissionRules)
            .await
    }

    pub async fn stop_task(&self, params: StopTaskParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::StopTask(params))
            .await
    }

    pub async fn task_list(&self) -> Result<TaskListResult, ClientError> {
        self.send_typed_client_request(ClientRequest::TaskList)
            .await
    }

    pub async fn task_detail(
        &self,
        params: TaskDetailParams,
    ) -> Result<TaskDetailResult, ClientError> {
        self.send_typed_client_request(ClientRequest::TaskDetail(params))
            .await
    }

    pub async fn background_all_tasks(&self) -> Result<BackgroundAllTasksResult, ClientError> {
        self.send_typed_client_request(ClientRequest::BackgroundAllTasks)
            .await
    }

    pub async fn rewind_files(
        &self,
        params: RewindFilesParams,
    ) -> Result<RewindFilesResult, ClientError> {
        self.send_typed_client_request(ClientRequest::RewindFiles(params))
            .await
    }

    pub async fn update_env(&self, params: UpdateEnvParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::UpdateEnv(params))
            .await
    }

    pub async fn cancel_request(&self, params: CancelRequestParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::CancelRequest(params))
            .await
    }

    pub async fn agent_interrupt_current_work(
        &self,
        params: AgentInterruptCurrentWorkParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::AgentInterruptCurrentWork(params))
            .await
    }

    pub async fn config_read(&self) -> Result<ConfigReadResult, ClientError> {
        self.send_typed_client_request(ClientRequest::ConfigRead)
            .await
    }

    pub async fn config_write(&self, params: ConfigWriteParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::ConfigWrite(params))
            .await
    }

    pub async fn mcp_status(&self) -> Result<McpStatusResult, ClientError> {
        self.send_typed_client_request(ClientRequest::McpStatus)
            .await
    }

    pub async fn context_usage(&self) -> Result<ContextUsageResult, ClientError> {
        self.send_typed_client_request(ClientRequest::ContextUsage)
            .await
    }

    pub async fn mcp_set_servers(
        &self,
        params: McpSetServersParams,
    ) -> Result<McpSetServersResult, ClientError> {
        self.send_typed_client_request(ClientRequest::McpSetServers(params))
            .await
    }

    pub async fn mcp_reconnect(&self, params: McpReconnectParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::McpReconnect(params))
            .await
    }

    pub async fn mcp_toggle(&self, params: McpToggleParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::McpToggle(params))
            .await
    }

    pub async fn plugin_reload(&self) -> Result<PluginReloadResult, ClientError> {
        self.send_typed_client_request(ClientRequest::PluginReload)
            .await
    }

    pub async fn hook_reload(&self) -> Result<HookReloadResult, ClientError> {
        self.send_typed_client_request(ClientRequest::HookReload)
            .await
    }

    pub async fn config_apply_flags(
        &self,
        params: ConfigApplyFlagsParams,
    ) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::ConfigApplyFlags(params))
            .await
    }

    pub async fn send_typed_client_request<T>(
        &self,
        request: ClientRequest,
    ) -> Result<T, ClientError>
    where
        T: serde::de::DeserializeOwned,
    {
        let result = self.send_client_request(request).await?;
        serde_json::from_value(result).map_err(|error| {
            ClientError::InvalidArgument(format!("failed to decode response result: {error}"))
        })
    }

    pub async fn request(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(ClientError::ClientInvalid);
        }

        let id = JsonRpcId::Number(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = lock_pending(&self.pending);
            if self.invalid.load(Ordering::Acquire) {
                return Err(ClientError::ClientInvalid);
            }
            pending.insert(id.clone(), PendingRemoteRequest { reply: tx });
        }

        let frame = JsonRpcFrame::Request(JsonRpcRequest::new(id.clone(), method, params));
        if self.outbound.send(frame).await.is_err() {
            lock_pending(&self.pending).remove(&id);
            return Err(ClientError::Disconnected);
        }

        let outcome = match self.request_timeout {
            None => rx.await,
            Some(request_timeout) => match tokio::time::timeout(request_timeout, rx).await {
                Ok(outcome) => outcome,
                Err(_elapsed) => {
                    lock_pending(&self.pending).remove(&id);
                    return Err(ClientError::Timeout);
                }
            },
        };
        match outcome {
            Ok(result) => result,
            Err(_) => Err(ClientError::Disconnected),
        }
    }

    pub async fn reply_server_request_success(
        &self,
        id: JsonRpcId,
        result: serde_json::Value,
    ) -> Result<(), ClientError> {
        self.send_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(id, result)))
            .await
    }

    pub async fn reply_server_request_error(
        &self,
        id: JsonRpcId,
        code: i32,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<(), ClientError> {
        self.send_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            id,
            JsonRpcErrorObject::new(code, message, data),
        )))
        .await
    }

    async fn send_frame(&self, frame: JsonRpcFrame) -> Result<(), ClientError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(ClientError::ClientInvalid);
        }
        self.outbound
            .send(frame)
            .await
            .map_err(|_| ClientError::Disconnected)
    }
}

impl RemoteSessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn surface_stream<'a>(&self, demux: &'a mut RemoteEventDemux) -> RemoteSurfaceStream<'a> {
        demux.surface_stream(self.surface_id.clone())
    }

    pub fn owned_surface_stream(&self, demux: RemoteEventDemux) -> RemoteOwnedSurfaceStream {
        demux.into_surface_stream(self.surface_id.clone())
    }

    pub fn try_next_event(&self, demux: &mut RemoteEventDemux) -> Option<SessionEnvelope> {
        demux.try_next_surface_event(&self.surface_id)
    }

    pub async fn next_event(&self, demux: &mut RemoteEventDemux) -> Option<SessionEnvelope> {
        demux.next_surface_event(&self.surface_id).await
    }

    pub fn try_next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleEffect> {
        demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleEffect> {
        demux.next_lifecycle(&self.surface_id).await
    }

    pub async fn query(&self, params: TurnStartParams) -> Result<TurnStartResult, ClientError> {
        self.client.turn_start(params).await
    }

    pub async fn interrupt(&self) -> Result<(), ClientError> {
        self.client.turn_interrupt().await
    }

    pub async fn replace_with_start(
        self,
        demux: &mut RemoteEventDemux,
        params: SessionStartParams,
    ) -> Result<Self, (Self, ClientError)> {
        let old_surface_id = self.surface_id.clone();
        match self.client.session_start(params).await {
            Ok(started) => {
                // Drop stale buffered deliveries for the replaced surface before
                // waiting for / minting the successor handle.
                demux.purge_surface(&old_surface_id);
                let surface_id = match started.surface_id {
                    Some(surface_id) => surface_id,
                    None => {
                        let Some(delivery) =
                            demux.next_session_activation(&started.session_id).await
                        else {
                            return Err((self, ClientError::Disconnected));
                        };
                        delivery.surface_id
                    }
                };
                Ok(self.client.session_handle(started.session_id, surface_id))
            }
            Err(error) => Err((self, error)),
        }
    }

    pub async fn replace_with_resume(
        self,
        demux: &mut RemoteEventDemux,
        params: SessionResumeParams,
    ) -> Result<Self, (Self, ClientError)> {
        let old_surface_id = self.surface_id.clone();
        match self.client.session_resume(params).await {
            Ok(resumed) => {
                // Drop stale buffered deliveries for the replaced surface before
                // waiting for / minting the successor handle.
                demux.purge_surface(&old_surface_id);
                let session_id = resumed.session.session_id;
                let surface_id = match resumed.surface_id {
                    Some(surface_id) => surface_id,
                    None => {
                        let Some(delivery) = demux.next_session_activation(&session_id).await
                        else {
                            return Err((self, ClientError::Disconnected));
                        };
                        delivery.surface_id
                    }
                };
                Ok(self.client.session_handle(session_id, surface_id))
            }
            Err(error) => Err((self, error)),
        }
    }

    pub async fn close(self) -> Result<(), (Self, ClientError)> {
        let params = SessionArchiveParams {
            session_id: self.session_id.clone(),
        };
        match self.client.session_archive(params).await {
            Ok(()) => Ok(()),
            Err(error) => Err((self, error)),
        }
    }
}

impl RemotePassiveSessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn replayed(&self) -> &[SessionEnvelope] {
        &self.replayed
    }

    pub fn surface_stream<'a>(&self, demux: &'a mut RemoteEventDemux) -> RemoteSurfaceStream<'a> {
        demux.surface_stream(self.surface_id.clone())
    }

    pub fn owned_surface_stream(&self, demux: RemoteEventDemux) -> RemoteOwnedSurfaceStream {
        demux.into_surface_stream(self.surface_id.clone())
    }

    pub fn try_next_event(&self, demux: &mut RemoteEventDemux) -> Option<SessionEnvelope> {
        demux.try_next_surface_event(&self.surface_id)
    }

    pub async fn next_event(&self, demux: &mut RemoteEventDemux) -> Option<SessionEnvelope> {
        demux.next_surface_event(&self.surface_id).await
    }

    pub fn try_next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleEffect> {
        demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleEffect> {
        demux.next_lifecycle(&self.surface_id).await
    }

    pub async fn read(
        &self,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionReadResult, ClientError> {
        self.client
            .session_read(SessionReadParams {
                session_id: self.session_id.clone(),
                cursor,
                limit,
            })
            .await
    }

    pub async fn turns(
        &self,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionTurnsListResult, ClientError> {
        self.client
            .session_turns_list(SessionTurnsListParams {
                session_id: self.session_id.clone(),
                cursor,
                limit,
            })
            .await
    }
}

impl RemoteJsonRpcIncoming {
    pub async fn handle_frame(&self, frame: JsonRpcFrame) -> Result<(), ClientError> {
        match frame {
            JsonRpcFrame::Success(success) => {
                self.resolve_success(success);
                Ok(())
            }
            JsonRpcFrame::Error(error) => {
                self.resolve_error(error);
                Ok(())
            }
            JsonRpcFrame::Notification(notification) => {
                // Fire-and-forget: an undecodable notification payload (unknown
                // effect kind / event layer on a newer server) is dropped with a
                // warning. Dropping one cannot corrupt request correlation.
                let Some(event) = remote_event_from_notification(notification) else {
                    return Ok(());
                };
                self.events
                    .send(event)
                    .await
                    .map_err(|_| ClientError::Disconnected)
            }
            JsonRpcFrame::Request(request) => self
                .events
                .send(RemoteJsonRpcEvent::ServerRequest(request))
                .await
                .map_err(|_| ClientError::Disconnected),
        }
    }

    pub async fn disconnect(&self) {
        if self.invalid.swap(true, Ordering::AcqRel) {
            return;
        }
        self.drain_pending_disconnected();
        let _ = self.events.send(RemoteJsonRpcEvent::Disconnected).await;
    }

    /// Resolve every in-flight RPC with `Disconnected`. Callers must have already
    /// won the `invalid` flag so this runs exactly once per connection.
    fn drain_pending_disconnected(&self) {
        let pending = {
            let mut pending = lock_pending(&self.pending);
            pending
                .drain()
                .map(|(_, pending)| pending)
                .collect::<Vec<_>>()
        };
        for pending in pending {
            let _ = pending.reply.send(Err(ClientError::Disconnected));
        }
    }

    /// Tolerate-with-warn: an unknown / late / duplicate / null response id is
    /// peer noise (e.g. a reply arriving after a per-request timeout), not
    /// connection corruption. Drop it instead of invalidating the connection.
    fn resolve_success(&self, success: JsonRpcSuccess) {
        let Some(pending) = lock_pending(&self.pending).remove(&success.id) else {
            tracing::warn!(id = ?success.id, "dropping JSON-RPC success for unknown response id");
            return;
        };
        let _ = pending.reply.send(Ok(success.result));
    }

    fn resolve_error(&self, error: JsonRpcErrorResponse) {
        let Some(pending) = lock_pending(&self.pending).remove(&error.id) else {
            tracing::warn!(id = ?error.id, "dropping JSON-RPC error for unknown response id");
            return;
        };
        let JsonRpcErrorObject {
            code,
            message,
            data,
        } = error.error;
        let _ = pending
            .reply
            .send(Err(ClientError::from_json_rpc_error(code, message, data)));
    }
}

impl Drop for RemoteJsonRpcIncoming {
    /// Safety net for the standard shutdown move: aborting/dropping the owner
    /// task drops this half without a graceful `disconnect().await`. Still honor
    /// the SDK dual-channel disconnect so in-flight RPCs (which may have
    /// `request_timeout: None`) do not hang forever.
    fn drop(&mut self) {
        if self.invalid.swap(true, Ordering::AcqRel) {
            return;
        }
        self.drain_pending_disconnected();
        // Best-effort terminal event; `try_send` because Drop has no async context.
        let _ = self.events.try_send(RemoteJsonRpcEvent::Disconnected);
    }
}

fn domain_error_kind(data: Option<&serde_json::Value>) -> Option<&str> {
    data.and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteTransportError {
    #[error("{source}")]
    Transport { source: TransportFrameError },
    #[error("{source}")]
    WebSocket {
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[error("failed to encode websocket JSON-RPC frame: {source}")]
    EncodeWebSocketFrame { source: serde_json::Error },
    #[error("failed to decode websocket JSON-RPC frame: {source}")]
    DecodeWebSocketFrame { source: serde_json::Error },
    #[error("outbound write timed out (slow consumer)")]
    SlowConsumer,
    #[error("{source}")]
    Client { source: ClientError },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("transport disconnected")]
    Disconnected,
    #[error("client invalid (reconnect and resume)")]
    ClientInvalid,
    #[error("invalid request: {message}")]
    InvalidRequest {
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("invalid params: {message}")]
    InvalidParams {
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("method not found: {message}")]
    MethodNotFound {
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("internal server error: {message}")]
    InternalServerError {
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("server error {code} ({kind}): {message}")]
    Domain {
        code: i32,
        kind: String,
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("surface limit reached: {message}")]
    SurfaceLimit {
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("server error {code}: {message}")]
    Server {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("request timed out")]
    Timeout,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("snapshot required before subscribing")]
    SnapshotRequired,
}

impl ClientError {
    fn from_json_rpc_error(code: i32, message: String, data: Option<serde_json::Value>) -> Self {
        match domain_error_kind(data.as_ref()) {
            Some("snapshot_required") => return Self::SnapshotRequired,
            Some("surface_limit") => {
                return Self::SurfaceLimit { message, data };
            }
            Some(kind) => {
                return Self::Domain {
                    code,
                    kind: kind.to_string(),
                    message,
                    data,
                };
            }
            None => {}
        }
        match code {
            coco_types::error_codes::INVALID_REQUEST => Self::InvalidRequest { message, data },
            coco_types::error_codes::INVALID_PARAMS => Self::InvalidParams { message, data },
            coco_types::error_codes::METHOD_NOT_FOUND => Self::MethodNotFound { message, data },
            coco_types::error_codes::INTERNAL_ERROR => Self::InternalServerError { message, data },
            _ => Self::Server {
                code,
                message,
                data,
            },
        }
    }
}

fn client_request_method_and_params(
    request: &ClientRequest,
) -> Result<(String, Option<serde_json::Value>), ClientError> {
    let value = serde_json::to_value(request).map_err(|error| {
        ClientError::InvalidArgument(format!("failed to encode client request: {error}"))
    })?;
    let serde_json::Value::Object(mut object) = value else {
        return Ok((request.method().as_str().to_string(), None));
    };
    let method = match object.remove("method") {
        Some(serde_json::Value::String(method)) => method,
        _ => request.method().as_str().to_string(),
    };
    let params = object.remove("params");
    Ok((method, params))
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
