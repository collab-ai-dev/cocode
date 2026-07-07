//! Typed AppServer client handles.
//!
//! This Phase A slice provides the local in-process shape of the two-level
//! client contract plus the transport-agnostic remote JSON-RPC core. Concrete
//! dialing and runtime-driving methods land later.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use coco_app_server::AttachError;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::DetachSurfaceOutcome;
use coco_app_server::DisconnectOutcome;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientConnection;
use coco_app_server::LocalClientDispatchError;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server::LocalClientSubscribeOutcome;
use coco_app_server::ServerRequestDelivery;
use coco_app_server::SessionSurfaceCounts;
use coco_app_server::SurfaceDelivery;
use coco_app_server::SurfaceLifecycleDelivery;
use coco_app_server::SurfaceLifecycleEffect;
use coco_app_server::SurfaceLifecycleEffectKind;
use coco_app_server::SurfaceRole;
use coco_app_server_transport::JsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcNotification;
use coco_app_server_transport::JsonRpcRequest;
use coco_app_server_transport::JsonRpcSuccess;
use coco_app_server_transport::NdjsonDuplexConnection;
use coco_app_server_transport::TransportFrameError;
use coco_types::AgentInterruptCurrentWorkParams;
use coco_types::AgentStreamEvent;
use coco_types::ApprovalResolveParams;
use coco_types::CancelRequestParams;
use coco_types::ClientRequest;
use coco_types::ConfigApplyFlagsParams;
use coco_types::ConfigReadResult;
use coco_types::ConfigWriteParams;
use coco_types::ContextUsageResult;
use coco_types::CoreEvent;
use coco_types::ElicitationResolveParams;
use coco_types::InitializeParams;
use coco_types::InitializeResult;
use coco_types::McpReconnectParams;
use coco_types::McpSetServersParams;
use coco_types::McpSetServersResult;
use coco_types::McpStatusResult;
use coco_types::McpToggleParams;
use coco_types::PluginReloadResult;
use coco_types::RewindFilesParams;
use coco_types::RewindFilesResult;
use coco_types::ServerNotification;
use coco_types::SessionArchiveParams;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionListResult;
use coco_types::SessionReadParams;
use coco_types::SessionReadResult;
use coco_types::SessionResumeParams;
use coco_types::SessionResumeResult;
use coco_types::SessionStartParams;
use coco_types::SessionStartResult;
use coco_types::SetModelParams;
use coco_types::SetPermissionModeParams;
use coco_types::SetThinkingParams;
use coco_types::StopTaskParams;
use coco_types::SurfaceId;
use coco_types::TuiOnlyEvent;
use coco_types::TurnStartParams;
use coco_types::TurnStartResult;
use coco_types::UpdateEnvParams;
use coco_types::UserInputResolveParams;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

const DEFAULT_REMOTE_EVENT_CHANNEL_CAPACITY: usize = 128;
const DEFAULT_REMOTE_OUTBOUND_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteConnectOptions {
    pub outbound_channel_capacity: usize,
    pub event_channel_capacity: usize,
}

impl Default for RemoteConnectOptions {
    fn default() -> Self {
        Self {
            outbound_channel_capacity: DEFAULT_REMOTE_OUTBOUND_CHANNEL_CAPACITY,
            event_channel_capacity: DEFAULT_REMOTE_EVENT_CHANNEL_CAPACITY,
        }
    }
}

pub struct ServerClient<H> {
    connection: LocalClientConnection<H>,
    event_buffers: HashMap<SurfaceId, VecDeque<SessionEnvelope>>,
    request_buffers: HashMap<SurfaceId, VecDeque<ServerRequestDelivery>>,
    lifecycle_buffers: HashMap<SurfaceId, VecDeque<SurfaceLifecycleDelivery>>,
}

impl<H: Clone> ServerClient<H> {
    pub fn connect_local(adapter: &LocalClientAdapter<H>) -> Self {
        Self {
            connection: adapter.connect(),
            event_buffers: HashMap::new(),
            request_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
        }
    }

    pub async fn send_client_request<Handler>(
        &self,
        handler: &Handler,
        request: ClientRequest,
    ) -> Result<serde_json::Value, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.connection
            .dispatch_client_request(handler, request)
            .await
            .map_err(ClientError::from)
    }

    pub async fn initialize<Handler>(
        &self,
        handler: &Handler,
        params: InitializeParams,
    ) -> Result<InitializeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::Initialize(params))
            .await
    }

    pub async fn keep_alive<Handler>(&self, handler: &Handler) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::KeepAlive)
            .await
    }

    pub async fn session_start<Handler>(
        &self,
        handler: &Handler,
        params: SessionStartParams,
    ) -> Result<SessionStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionStart(Box::new(params)))
            .await
    }

    pub async fn session_resume<Handler>(
        &self,
        handler: &Handler,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionResume(params))
            .await
    }

    pub async fn session_list<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<SessionListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionList)
            .await
    }

    pub async fn session_read<Handler>(
        &self,
        handler: &Handler,
        params: SessionReadParams,
    ) -> Result<SessionReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionRead(params))
            .await
    }

    pub async fn session_archive<Handler>(
        &self,
        handler: &Handler,
        params: SessionArchiveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionArchive(params))
            .await
    }

    pub async fn turn_start<Handler>(
        &self,
        handler: &Handler,
        params: TurnStartParams,
    ) -> Result<TurnStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TurnStart(params))
            .await
    }

    pub async fn turn_interrupt<Handler>(&self, handler: &Handler) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TurnInterrupt)
            .await
    }

    pub async fn approval_resolve<Handler>(
        &self,
        handler: &Handler,
        params: ApprovalResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ApprovalResolve(params))
            .await
    }

    pub async fn user_input_resolve<Handler>(
        &self,
        handler: &Handler,
        params: UserInputResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::UserInputResolve(params))
            .await
    }

    pub async fn elicitation_resolve<Handler>(
        &self,
        handler: &Handler,
        params: ElicitationResolveParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ElicitationResolve(params))
            .await
    }

    pub async fn set_model<Handler>(
        &self,
        handler: &Handler,
        params: SetModelParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetModel(params))
            .await
    }

    pub async fn set_permission_mode<Handler>(
        &self,
        handler: &Handler,
        params: SetPermissionModeParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetPermissionMode(params))
            .await
    }

    pub async fn set_thinking<Handler>(
        &self,
        handler: &Handler,
        params: SetThinkingParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetThinking(params))
            .await
    }

    pub async fn stop_task<Handler>(
        &self,
        handler: &Handler,
        params: StopTaskParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::StopTask(params))
            .await
    }

    pub async fn rewind_files<Handler>(
        &self,
        handler: &Handler,
        params: RewindFilesParams,
    ) -> Result<RewindFilesResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::RewindFiles(params))
            .await
    }

    pub async fn update_env<Handler>(
        &self,
        handler: &Handler,
        params: UpdateEnvParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::UpdateEnv(params))
            .await
    }

    pub async fn cancel_request<Handler>(
        &self,
        handler: &Handler,
        params: CancelRequestParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::CancelRequest(params))
            .await
    }

    pub async fn agent_interrupt_current_work<Handler>(
        &self,
        handler: &Handler,
        params: AgentInterruptCurrentWorkParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::AgentInterruptCurrentWork(params))
            .await
    }

    pub async fn config_read<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<ConfigReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigRead)
            .await
    }

    pub async fn config_write<Handler>(
        &self,
        handler: &Handler,
        params: ConfigWriteParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigWrite(params))
            .await
    }

    pub async fn mcp_status<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<McpStatusResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpStatus)
            .await
    }

    pub async fn context_usage<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<ContextUsageResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ContextUsage)
            .await
    }

    pub async fn mcp_set_servers<Handler>(
        &self,
        handler: &Handler,
        params: McpSetServersParams,
    ) -> Result<McpSetServersResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpSetServers(params))
            .await
    }

    pub async fn mcp_reconnect<Handler>(
        &self,
        handler: &Handler,
        params: McpReconnectParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpReconnect(params))
            .await
    }

    pub async fn mcp_toggle<Handler>(
        &self,
        handler: &Handler,
        params: McpToggleParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::McpToggle(params))
            .await
    }

    pub async fn plugin_reload<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<PluginReloadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::PluginReload)
            .await
    }

    pub async fn config_apply_flags<Handler>(
        &self,
        handler: &Handler,
        params: ConfigApplyFlagsParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ConfigApplyFlags(params))
            .await
    }

    pub async fn send_typed_client_request<Handler, T>(
        &self,
        handler: &Handler,
        request: ClientRequest,
    ) -> Result<T, ClientError>
    where
        Handler: LocalClientRequestHandler,
        T: serde::de::DeserializeOwned,
    {
        let result = self.send_client_request(handler, request).await?;
        serde_json::from_value(result).map_err(|error| {
            ClientError::InvalidArgument(format!("failed to decode response result: {error}"))
        })
    }

    pub fn attach_interactive_session(
        &self,
        session_id: SessionId,
        mut options: AttachSurfaceOptions,
    ) -> Result<SessionClient, ClientError> {
        options.role = SurfaceRole::Interactive;
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(ClientError::from)?;
        Ok(SessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
        })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<PassiveSessionClient, ClientError> {
        let subscription = self
            .connection
            .subscribe_surface(session_id, after_seq, options)
            .map_err(ClientError::from)?;
        match subscription {
            LocalClientSubscribeOutcome::Attached(subscription) => Ok(PassiveSessionClient {
                session_id: subscription.session_id,
                surface_id: subscription.surface_id,
                replayed: subscription.replayed,
            }),
            LocalClientSubscribeOutcome::SnapshotRequired => Err(ClientError::SnapshotRequired),
        }
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceDelivery> {
        self.connection.events_mut()
    }

    pub fn try_next_session_event(&mut self, session: &SessionClient) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_session_event(&mut self, session: &SessionClient) -> Option<SessionEnvelope> {
        self.next_event_for_surface(session.surface_id()).await
    }

    pub fn try_next_passive_event(
        &mut self,
        session: &PassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_passive_event(
        &mut self,
        session: &PassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.next_event_for_surface(session.surface_id()).await
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        self.connection.server_requests_mut()
    }

    pub fn try_next_session_request(
        &mut self,
        session: &SessionClient,
    ) -> Option<ServerRequestDelivery> {
        self.try_next_request_for_surface(session.surface_id())
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery> {
        self.connection.lifecycle_mut()
    }

    pub fn try_next_session_lifecycle(
        &mut self,
        session: &SessionClient,
    ) -> Option<SurfaceLifecycleDelivery> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn try_next_passive_lifecycle(
        &mut self,
        session: &PassiveSessionClient,
    ) -> Option<SurfaceLifecycleDelivery> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn detach_passive(
        &self,
        passive: PassiveSessionClient,
    ) -> Result<DetachSurfaceOutcome, (PassiveSessionClient, ClientError)> {
        let outcome = self.connection.detach_surface(&passive.surface_id);
        if outcome.detached_surface.is_some() {
            Ok(outcome)
        } else {
            Err((
                passive,
                ClientError::InvalidArgument("passive surface is not attached".to_string()),
            ))
        }
    }

    pub fn close(self) -> Result<DisconnectOutcome, ClientError> {
        Ok(self.connection.disconnect())
    }

    pub fn list_live_sessions(&self) -> Vec<LiveSessionSummary> {
        self.connection
            .list_live_sessions()
            .into_iter()
            .map(|summary| LiveSessionSummary {
                session_id: summary.session_id,
                surface_counts: summary.surface_counts,
            })
            .collect()
    }

    fn try_next_event_for_surface(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            let delivery = self.connection.events_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery.envelope);
            }
            self.event_buffers
                .entry(delivery.surface_id)
                .or_default()
                .push_back(delivery.envelope);
        }
    }

    async fn next_event_for_surface(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            let delivery = self.connection.events_mut().recv().await?;
            if &delivery.surface_id == surface_id {
                return Some(delivery.envelope);
            }
            self.event_buffers
                .entry(delivery.surface_id)
                .or_default()
                .push_back(delivery.envelope);
        }
    }

    fn pop_buffered_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        let queue = self.event_buffers.get_mut(surface_id)?;
        let envelope = queue.pop_front();
        if queue.is_empty() {
            self.event_buffers.remove(surface_id);
        }
        envelope
    }

    fn try_next_request_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<ServerRequestDelivery> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.request_buffers, surface_id) {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.server_requests_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.request_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn try_next_lifecycle_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleDelivery> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.lifecycle_buffers, surface_id)
        {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.lifecycle_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.lifecycle_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn pop_buffered_delivery<T>(
        buffers: &mut HashMap<SurfaceId, VecDeque<T>>,
        surface_id: &SurfaceId,
    ) -> Option<T> {
        let queue = buffers.get_mut(surface_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            buffers.remove(surface_id);
        }
        delivery
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
}

impl SessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }
}

#[derive(Debug, Clone)]
pub struct PassiveSessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
    replayed: Vec<SessionEnvelope>,
}

impl PassiveSessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn replayed(&self) -> &[SessionEnvelope] {
        &self.replayed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSessionSummary {
    pub session_id: SessionId,
    pub surface_counts: SessionSurfaceCounts,
}

#[derive(Clone)]
pub struct RemoteJsonRpcClient {
    outbound: mpsc::Sender<JsonRpcFrame>,
    pending: Arc<Mutex<HashMap<JsonRpcId, PendingRemoteRequest>>>,
    invalid: Arc<AtomicBool>,
    next_request_id: Arc<AtomicI64>,
}

pub struct RemoteJsonRpcIncoming {
    pending: Arc<Mutex<HashMap<JsonRpcId, PendingRemoteRequest>>>,
    events: mpsc::Sender<RemoteJsonRpcEvent>,
    invalid: Arc<AtomicBool>,
}

pub struct RemoteNdjsonConnection<R, W> {
    incoming: RemoteJsonRpcIncoming,
    outbound: mpsc::Receiver<JsonRpcFrame>,
    transport: NdjsonDuplexConnection<R, W>,
}

#[cfg(unix)]
pub type RemoteNdjsonUnixConnection = RemoteNdjsonConnection<
    tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
>;

pub struct RemoteEventDemux {
    events: mpsc::Receiver<RemoteJsonRpcEvent>,
    event_buffers: HashMap<SurfaceId, VecDeque<SessionEnvelope>>,
    lifecycle_buffers: HashMap<SurfaceId, VecDeque<SurfaceLifecycleDelivery>>,
    server_requests: VecDeque<JsonRpcRequest>,
    notifications: VecDeque<JsonRpcNotification>,
    disconnected: bool,
}

pub struct RemoteSurfaceStream<'a> {
    demux: &'a mut RemoteEventDemux,
    surface_id: SurfaceId,
}

#[derive(Debug, Clone)]
pub enum RemoteJsonRpcEvent {
    SurfaceDelivery(Box<SurfaceDelivery>),
    SurfaceLifecycle(SurfaceLifecycleDelivery),
    Notification(JsonRpcNotification),
    ServerRequest(JsonRpcRequest),
    Disconnected,
}

struct PendingRemoteRequest {
    reply: oneshot::Sender<Result<serde_json::Value, ClientError>>,
}

impl RemoteEventDemux {
    pub fn new(events: mpsc::Receiver<RemoteJsonRpcEvent>) -> Self {
        Self {
            events,
            event_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
            server_requests: VecDeque::new(),
            notifications: VecDeque::new(),
            disconnected: false,
        }
    }

    pub fn try_next_surface_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery.envelope);
                    }
                    self.event_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery.envelope);
                }
                event => self.buffer_non_surface_event(event),
            }
        }
    }

    pub async fn next_surface_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery.envelope);
                    }
                    self.event_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery.envelope);
                }
                event => self.buffer_non_surface_event(event),
            }
        }
    }

    pub fn try_next_lifecycle(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleDelivery> {
        if let Some(delivery) = self.pop_buffered_lifecycle(surface_id) {
            return Some(delivery);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    pub async fn next_lifecycle(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleDelivery> {
        if let Some(delivery) = self.pop_buffered_lifecycle(surface_id) {
            return Some(delivery);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    pub fn try_next_server_request(&mut self) -> Option<JsonRpcRequest> {
        if let Some(request) = self.server_requests.pop_front() {
            return Some(request);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::ServerRequest(request) => return Some(request),
                event => self.buffer_non_server_request_event(event),
            }
        }
    }

    pub async fn next_server_request(&mut self) -> Option<JsonRpcRequest> {
        if let Some(request) = self.server_requests.pop_front() {
            return Some(request);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::ServerRequest(request) => return Some(request),
                event => self.buffer_non_server_request_event(event),
            }
        }
    }

    pub fn try_next_notification(&mut self) -> Option<JsonRpcNotification> {
        if let Some(notification) = self.notifications.pop_front() {
            return Some(notification);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::Notification(notification) => return Some(notification),
                event => self.buffer_non_notification_event(event),
            }
        }
    }

    pub async fn next_notification(&mut self) -> Option<JsonRpcNotification> {
        if let Some(notification) = self.notifications.pop_front() {
            return Some(notification);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::Notification(notification) => return Some(notification),
                event => self.buffer_non_notification_event(event),
            }
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn surface_stream(&mut self, surface_id: SurfaceId) -> RemoteSurfaceStream<'_> {
        RemoteSurfaceStream {
            demux: self,
            surface_id,
        }
    }

    fn next_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        match self.events.try_recv() {
            Ok(RemoteJsonRpcEvent::Disconnected) => {
                self.disconnected = true;
                None
            }
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.disconnected = true;
                None
            }
        }
    }

    async fn recv_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        match self.events.recv().await {
            Some(RemoteJsonRpcEvent::Disconnected) => {
                self.disconnected = true;
                None
            }
            Some(event) => Some(event),
            None => {
                self.disconnected = true;
                None
            }
        }
    }

    fn pop_buffered_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        let queue = self.event_buffers.get_mut(surface_id)?;
        let envelope = queue.pop_front();
        if queue.is_empty() {
            self.event_buffers.remove(surface_id);
        }
        envelope
    }

    fn pop_buffered_lifecycle(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleDelivery> {
        let queue = self.lifecycle_buffers.get_mut(surface_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            self.lifecycle_buffers.remove(surface_id);
        }
        delivery
    }

    fn buffer_non_surface_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_lifecycle_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_server_request_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_notification_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            RemoteJsonRpcEvent::ServerRequest(request) => {
                self.server_requests.push_back(request);
            }
            RemoteJsonRpcEvent::Disconnected => {
                self.disconnected = true;
            }
            RemoteJsonRpcEvent::Notification(_) => {}
        }
    }

    fn buffer_common_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::ServerRequest(request) => {
                self.server_requests.push_back(request);
            }
            RemoteJsonRpcEvent::Notification(notification) => {
                self.notifications.push_back(notification);
            }
            RemoteJsonRpcEvent::Disconnected => {
                self.disconnected = true;
            }
            RemoteJsonRpcEvent::SurfaceDelivery(_) | RemoteJsonRpcEvent::SurfaceLifecycle(_) => {}
        }
    }
}

impl RemoteSurfaceStream<'_> {
    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn try_next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.try_next_surface_event(&self.surface_id)
    }

    pub async fn next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.next_surface_event(&self.surface_id).await
    }

    pub fn try_next_lifecycle(&mut self) -> Option<SurfaceLifecycleDelivery> {
        self.demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(&mut self) -> Option<SurfaceLifecycleDelivery> {
        self.demux.next_lifecycle(&self.surface_id).await
    }
}

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
        Self::connect_ndjson_with_channel_capacity(
            transport,
            options.outbound_channel_capacity,
            options.event_channel_capacity,
        )
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
        assert!(
            outbound_channel_capacity > 0,
            "remote outbound channel capacity must be non-zero"
        );
        let (outbound_tx, outbound_rx) = mpsc::channel(outbound_channel_capacity);
        let (client, incoming, events) =
            Self::with_event_channel_capacity(outbound_tx, event_channel_capacity);
        let connection = RemoteNdjsonConnection {
            incoming,
            outbound: outbound_rx,
            transport,
        };
        (client, connection, events)
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
        TransportFrameError,
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
        TransportFrameError,
    > {
        Self::connect_unix_with_channel_capacity(
            path,
            options.outbound_channel_capacity,
            options.event_channel_capacity,
        )
        .await
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
        TransportFrameError,
    > {
        let transport = coco_app_server_transport::connect_ndjson_unix(path).await?;
        Ok(Self::connect_ndjson_with_channel_capacity(
            transport,
            outbound_channel_capacity,
            event_channel_capacity,
        ))
    }

    pub async fn send_client_request(
        &self,
        request: ClientRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let (method, params) = client_request_method_and_params(&request)?;
        self.request(method, params).await
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

    pub async fn session_resume(
        &self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult, ClientError> {
        self.send_typed_client_request(ClientRequest::SessionResume(params))
            .await
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

    pub async fn session_archive(&self, params: SessionArchiveParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::SessionArchive(params))
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

    pub async fn stop_task(&self, params: StopTaskParams) -> Result<(), ClientError> {
        self.send_typed_client_request(ClientRequest::StopTask(params))
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
            let mut pending = self.pending.lock().await;
            if self.invalid.load(Ordering::Acquire) {
                return Err(ClientError::ClientInvalid);
            }
            pending.insert(id.clone(), PendingRemoteRequest { reply: tx });
        }

        let frame = JsonRpcFrame::Request(JsonRpcRequest::new(id.clone(), method, params));
        if self.outbound.send(frame).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(ClientError::Disconnected);
        }

        match rx.await {
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

impl RemoteJsonRpcIncoming {
    pub async fn handle_frame(&self, frame: JsonRpcFrame) -> Result<(), ClientError> {
        match frame {
            JsonRpcFrame::Success(success) => self.resolve_success(success).await,
            JsonRpcFrame::Error(error) => self.resolve_error(error).await,
            JsonRpcFrame::Notification(notification) => {
                let event = remote_event_from_notification(notification)?;
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
        let pending = {
            let mut pending = self.pending.lock().await;
            pending
                .drain()
                .map(|(_, pending)| pending)
                .collect::<Vec<_>>()
        };
        for pending in pending {
            let _ = pending.reply.send(Err(ClientError::Disconnected));
        }
        let _ = self.events.send(RemoteJsonRpcEvent::Disconnected).await;
    }

    async fn resolve_success(&self, success: JsonRpcSuccess) -> Result<(), ClientError> {
        let Some(pending) = self.pending.lock().await.remove(&success.id) else {
            return Err(ClientError::InvalidArgument(
                "unknown JSON-RPC response id".to_string(),
            ));
        };
        let _ = pending.reply.send(Ok(success.result));
        Ok(())
    }

    async fn resolve_error(&self, error: JsonRpcErrorResponse) -> Result<(), ClientError> {
        let Some(pending) = self.pending.lock().await.remove(&error.id) else {
            return Err(ClientError::InvalidArgument(
                "unknown JSON-RPC response id".to_string(),
            ));
        };
        let JsonRpcErrorObject {
            code,
            message,
            data,
        } = error.error;
        let _ = pending.reply.send(Err(ClientError::Server {
            code,
            message,
            data,
        }));
        Ok(())
    }
}

impl<R, W> RemoteNdjsonConnection<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub async fn run(self) -> Result<(), RemoteTransportError> {
        let RemoteNdjsonConnection {
            incoming,
            mut outbound,
            transport,
        } = self;
        let (mut reader, mut writer) = transport.split();
        let result = loop {
            tokio::select! {
                frame = reader.read_frame() => {
                    match frame.map_err(|source| RemoteTransportError::Transport { source })? {
                        Some(frame) => incoming.handle_frame(frame).await.map_err(|source| RemoteTransportError::Client { source })?,
                        None => break Ok(()),
                    }
                }
                frame = outbound.recv() => {
                    let Some(frame) = frame else {
                        break Ok(());
                    };
                    writer
                        .write_frame(&frame)
                        .await
                        .map_err(|source| RemoteTransportError::Transport { source })?;
                }
            }
        };
        incoming.disconnect().await;
        result
    }
}

fn remote_event_from_notification(
    notification: JsonRpcNotification,
) -> Result<RemoteJsonRpcEvent, ClientError> {
    match notification.method.as_str() {
        "session/event" => Ok(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
            decode_surface_delivery_notification(notification.params)?,
        ))),
        "session/lifecycle" => Ok(RemoteJsonRpcEvent::SurfaceLifecycle(
            decode_lifecycle_notification(notification.params)?,
        )),
        _ => Ok(RemoteJsonRpcEvent::Notification(notification)),
    }
}

fn decode_surface_delivery_notification(
    params: Option<serde_json::Value>,
) -> Result<SurfaceDelivery, ClientError> {
    let mut params = object_params(params, "session/event")?;
    let surface_id = take_field(&mut params, "surface_id", "session/event")?;
    let mut envelope = take_object_field(&mut params, "envelope", "session/event")?;
    let session_id = take_field(&mut envelope, "session_id", "session/event envelope")?;
    let agent_id = take_optional_field(&mut envelope, "agent_id", "session/event envelope")?;
    let turn_id = take_optional_field(&mut envelope, "turn_id", "session/event envelope")?;
    let session_seq = take_optional_field(&mut envelope, "session_seq", "session/event envelope")?;
    let event = decode_core_event(take_object_field(
        &mut envelope,
        "event",
        "session/event envelope",
    )?)?;
    Ok(SurfaceDelivery {
        surface_id,
        envelope: SessionEnvelope {
            session_id,
            agent_id,
            turn_id,
            session_seq,
            event,
        },
    })
}

fn decode_lifecycle_notification(
    params: Option<serde_json::Value>,
) -> Result<SurfaceLifecycleDelivery, ClientError> {
    let mut params = object_params(params, "session/lifecycle")?;
    let surface_id: SurfaceId = take_field(&mut params, "surface_id", "session/lifecycle")?;
    let mut effect = take_object_field(&mut params, "effect", "session/lifecycle")?;
    let effect_type: String = take_field(&mut effect, "type", "session/lifecycle effect")?;
    let kind = match effect_type.as_str() {
        "session_started" => SurfaceLifecycleEffectKind::SessionStarted {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        "session_replaced" => SurfaceLifecycleEffectKind::SessionReplaced {
            old_session_id: take_field(&mut effect, "old_session_id", "session/lifecycle effect")?,
            new_session_id: take_field(&mut effect, "new_session_id", "session/lifecycle effect")?,
        },
        "session_ended" => SurfaceLifecycleEffectKind::SessionEnded {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        other => {
            return Err(ClientError::InvalidArgument(format!(
                "unknown session/lifecycle effect type: {other}"
            )));
        }
    };
    Ok(SurfaceLifecycleDelivery {
        surface_id: surface_id.clone(),
        effect: SurfaceLifecycleEffect { surface_id, kind },
    })
}

fn decode_core_event(
    mut event: serde_json::Map<String, serde_json::Value>,
) -> Result<CoreEvent, ClientError> {
    let layer: String = take_field(&mut event, "layer", "session/event core event")?;
    let payload = event
        .remove("payload")
        .ok_or_else(|| ClientError::InvalidArgument("missing session/event payload".to_string()))?;
    match layer.as_str() {
        "protocol" => serde_json::from_value::<ServerNotification>(payload)
            .map(CoreEvent::Protocol)
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid protocol event: {error}"))
            }),
        "stream" => serde_json::from_value::<AgentStreamEvent>(payload)
            .map(CoreEvent::Stream)
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid stream event: {error}"))
            }),
        "tui" => serde_json::from_value::<TuiOnlyEvent>(payload)
            .map(CoreEvent::Tui)
            .map_err(|error| ClientError::InvalidArgument(format!("invalid tui event: {error}"))),
        other => Err(ClientError::InvalidArgument(format!(
            "unknown session/event layer: {other}"
        ))),
    }
}

fn object_params(
    params: Option<serde_json::Value>,
    context: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ClientError> {
    match params {
        Some(serde_json::Value::Object(object)) => Ok(object),
        _ => Err(ClientError::InvalidArgument(format!(
            "{context} params must be an object"
        ))),
    }
}

fn take_object_field(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ClientError> {
    match object.remove(field) {
        Some(serde_json::Value::Object(object)) => Ok(object),
        _ => Err(ClientError::InvalidArgument(format!(
            "missing or invalid {context}.{field}"
        ))),
    }
}

fn take_field<T>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<T, ClientError>
where
    T: serde::de::DeserializeOwned,
{
    let value = object
        .remove(field)
        .ok_or_else(|| ClientError::InvalidArgument(format!("missing {context}.{field}")))?;
    serde_json::from_value(value).map_err(|error| {
        ClientError::InvalidArgument(format!("invalid {context}.{field}: {error}"))
    })
}

fn take_optional_field<T>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<Option<T>, ClientError>
where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = object.remove(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value).map(Some).map_err(|error| {
        ClientError::InvalidArgument(format!("invalid {context}.{field}: {error}"))
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteTransportError {
    #[error("{source}")]
    Transport { source: TransportFrameError },
    #[error("{source}")]
    Client { source: ClientError },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport disconnected")]
    Disconnected,
    #[error("client invalid (reconnect and resume)")]
    ClientInvalid,
    #[error("server error {code}: {message}")]
    Server {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("snapshot required before subscribing")]
    SnapshotRequired,
}

impl From<AttachError> for ClientError {
    fn from(error: AttachError) -> Self {
        Self::InvalidArgument(error.to_string())
    }
}

impl From<LocalClientDispatchError> for ClientError {
    fn from(error: LocalClientDispatchError) -> Self {
        Self::Server {
            code: error.code,
            message: error.message,
            data: error.data,
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
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use coco_app_server::AppServer;
    use coco_app_server::ConnectionKey;
    use coco_app_server::LocalClientAdapter;
    use coco_app_server::LocalClientDispatchError;
    use coco_app_server::LocalClientRequestContext;
    use coco_app_server::LocalClientRequestFuture;
    use coco_app_server::LocalClientRequestHandler;
    use coco_app_server::SurfaceCapabilities;
    use coco_app_server::SurfaceCapability;
    use coco_app_server::SurfaceLifecycleEffect;
    use coco_app_server::SurfaceLifecycleEffectKind;
    use coco_types::CoreEvent;
    use coco_types::ServerNotification;
    use coco_types::ServerRequest;
    use coco_types::ServerRequestUserInputParams;
    use coco_types::SessionEnvelope;
    use coco_types::SessionState;
    use coco_types::TurnId;
    use tokio::io::BufReader;
    use tokio::io::split;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestHandle(&'static str);

    fn test_session_id(value: &str) -> SessionId {
        SessionId::try_new(value).expect("valid test session id")
    }

    fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
        SessionEnvelope::durable(
            session_id,
            None,
            None,
            seq,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            }),
        )
    }

    fn test_server_request(label: &str) -> ServerRequest {
        ServerRequest::RequestUserInput(ServerRequestUserInputParams {
            request_id: format!("payload-request-{label}"),
            prompt: "continue?".to_string(),
            description: None,
            choices: Vec::new(),
            default: None,
        })
    }

    struct RecordingLocalRequestHandler {
        calls: Arc<Mutex<Vec<(ConnectionKey, String)>>>,
        result: serde_json::Value,
        error: Option<LocalClientDispatchError>,
    }

    impl Default for RecordingLocalRequestHandler {
        fn default() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                result: serde_json::Value::Null,
                error: None,
            }
        }
    }

    impl LocalClientRequestHandler for RecordingLocalRequestHandler {
        fn handle_local_client_request(
            &self,
            context: LocalClientRequestContext,
            request: ClientRequest,
        ) -> LocalClientRequestFuture {
            let calls = Arc::clone(&self.calls);
            let result = self.result.clone();
            let error = self.error.clone();
            Box::pin(async move {
                calls.lock().expect("calls lock").push((
                    context.connection_key(),
                    request.method().as_str().to_string(),
                ));
                match error {
                    Some(error) => Err(error),
                    None => Ok(result),
                }
            })
        }
    }

    #[tokio::test]
    async fn local_server_client_typed_methods_dispatch_and_decode_results() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let session_id = test_session_id("sess-local-typed-client");
        let handler = RecordingLocalRequestHandler {
            result: serde_json::json!({ "session_id": session_id }),
            ..RecordingLocalRequestHandler::default()
        };

        let result = client
            .session_start(&handler, SessionStartParams::default())
            .await
            .expect("session start succeeds");

        assert_eq!(
            result.session_id,
            test_session_id("sess-local-typed-client")
        );
        {
            let calls = handler.calls.lock().expect("calls lock");
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, "session/start");
        }

        let unit_handler = RecordingLocalRequestHandler::default();
        client
            .user_input_resolve(
                &unit_handler,
                UserInputResolveParams {
                    request_id: "input-1".to_string(),
                    answer: "yes".to_string(),
                },
            )
            .await
            .expect("user input resolve succeeds");
        let calls = unit_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "input/resolveUserInput");
    }

    #[tokio::test]
    async fn local_server_client_maps_dispatch_errors_to_server_errors() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let handler = RecordingLocalRequestHandler {
            error: Some(LocalClientDispatchError::invalid_params(
                "bad local request",
            )),
            ..RecordingLocalRequestHandler::default()
        };

        let Err(ClientError::Server { message, .. }) = client.keep_alive(&handler).await else {
            panic!("expected server error");
        };

        assert_eq!(message, "bad local request");
    }

    #[tokio::test]
    async fn remote_json_rpc_client_correlates_success_response() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

        let request_task = tokio::spawn(async move {
            client
                .send_client_request(ClientRequest::KeepAlive)
                .await
                .expect("request succeeds")
        });
        let frame = outbound_rx.recv().await.expect("outbound request");
        let JsonRpcFrame::Request(request) = frame else {
            panic!("expected request frame");
        };
        assert_eq!(request.method, "control/keepAlive");

        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "ok": true }),
            )))
            .await
            .expect("handle success");

        assert_eq!(
            request_task.await.expect("request task"),
            serde_json::json!({ "ok": true })
        );
    }

    #[tokio::test]
    async fn remote_json_rpc_client_typed_methods_encode_and_decode_results() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);
        let start_client = client.clone();

        let start_task = tokio::spawn(async move {
            start_client
                .session_start(SessionStartParams::default())
                .await
                .expect("session start succeeds")
        });
        let JsonRpcFrame::Request(start_request) =
            outbound_rx.recv().await.expect("outbound session/start")
        else {
            panic!("expected request frame");
        };
        assert_eq!(start_request.method, "session/start");
        let session_id = test_session_id("sess-typed-client");
        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                start_request.id,
                serde_json::json!({ "session_id": session_id }),
            )))
            .await
            .expect("handle session/start response");
        assert_eq!(
            start_task.await.expect("start task").session_id,
            test_session_id("sess-typed-client")
        );

        let interrupt_client = client.clone();
        let interrupt_task = tokio::spawn(async move { interrupt_client.turn_interrupt().await });
        let JsonRpcFrame::Request(interrupt_request) =
            outbound_rx.recv().await.expect("outbound turn/interrupt")
        else {
            panic!("expected request frame");
        };
        assert_eq!(interrupt_request.method, "turn/interrupt");
        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                interrupt_request.id,
                serde_json::Value::Null,
            )))
            .await
            .expect("handle turn/interrupt response");
        interrupt_task
            .await
            .expect("interrupt task")
            .expect("interrupt succeeds");

        let input_client = client.clone();
        let input_task = tokio::spawn(async move {
            input_client
                .user_input_resolve(UserInputResolveParams {
                    request_id: "input-1".to_string(),
                    answer: "yes".to_string(),
                })
                .await
        });
        let JsonRpcFrame::Request(input_request) =
            outbound_rx.recv().await.expect("outbound input/resolve")
        else {
            panic!("expected request frame");
        };
        assert_eq!(input_request.method, "input/resolveUserInput");
        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                input_request.id,
                serde_json::Value::Null,
            )))
            .await
            .expect("handle input/resolve response");
        input_task
            .await
            .expect("input task")
            .expect("input resolve succeeds");

        let status_client = client.clone();
        let status_task = tokio::spawn(async move { status_client.mcp_status().await });
        let JsonRpcFrame::Request(status_request) =
            outbound_rx.recv().await.expect("outbound mcp/status")
        else {
            panic!("expected request frame");
        };
        assert_eq!(status_request.method, "mcp/status");
        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                status_request.id,
                serde_json::json!({ "mcpServers": [] }),
            )))
            .await
            .expect("handle mcp/status response");
        assert!(
            status_task
                .await
                .expect("status task")
                .expect("status succeeds")
                .mcp_servers
                .is_empty()
        );

        let apply_client = client.clone();
        let apply_task = tokio::spawn(async move {
            apply_client
                .config_apply_flags(ConfigApplyFlagsParams {
                    settings: HashMap::new(),
                })
                .await
        });
        let JsonRpcFrame::Request(apply_request) = outbound_rx
            .recv()
            .await
            .expect("outbound config/applyFlags")
        else {
            panic!("expected request frame");
        };
        assert_eq!(apply_request.method, "config/applyFlags");
        incoming
            .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
                apply_request.id,
                serde_json::Value::Null,
            )))
            .await
            .expect("handle config/applyFlags response");
        apply_task
            .await
            .expect("apply task")
            .expect("apply succeeds");
    }

    #[tokio::test]
    async fn remote_json_rpc_client_routes_server_error_to_pending_request() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (client, incoming, _events) = RemoteJsonRpcClient::new(outbound_tx);

        let request_task = tokio::spawn(async move {
            client
                .request(
                    "session/read",
                    Some(serde_json::json!({ "session_id": "sess-1" })),
                )
                .await
        });
        let JsonRpcFrame::Request(request) = outbound_rx.recv().await.expect("outbound request")
        else {
            panic!("expected request frame");
        };
        incoming
            .handle_frame(JsonRpcFrame::Error(JsonRpcErrorResponse::new(
                request.id,
                JsonRpcErrorObject::new(-32602, "bad params", None),
            )))
            .await
            .expect("handle error");

        let Err(ClientError::Server { code, message, .. }) =
            request_task.await.expect("request task")
        else {
            panic!("expected server error");
        };
        assert_eq!(code, -32602);
        assert_eq!(message, "bad params");
    }

    #[tokio::test]
    async fn remote_json_rpc_client_delivers_notifications_and_disconnect_terminal_event() {
        let (outbound_tx, _outbound_rx) = mpsc::channel(8);
        let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

        incoming
            .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
                "custom/notice",
                Some(serde_json::json!({ "surface_id": "surface-1" })),
            )))
            .await
            .expect("handle notification");
        assert!(matches!(
            events.recv().await.expect("notification event"),
            RemoteJsonRpcEvent::Notification(notification)
                if notification.method == "custom/notice"
        ));

        incoming.disconnect().await;
        assert!(matches!(
            events.recv().await.expect("disconnect event"),
            RemoteJsonRpcEvent::Disconnected
        ));
    }

    #[tokio::test]
    async fn remote_json_rpc_client_decodes_surface_delivery_notifications() {
        let (outbound_tx, _outbound_rx) = mpsc::channel(8);
        let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);
        let session_id = test_session_id("sess-remote");
        let notification = ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        };

        incoming
            .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
                "session/event",
                Some(serde_json::json!({
                    "surface_id": "surface-remote",
                    "envelope": {
                        "session_id": session_id,
                        "agent_id": null,
                        "turn_id": null,
                        "session_seq": 5,
                        "event": {
                            "layer": "protocol",
                            "payload": notification,
                        },
                    },
                })),
            )))
            .await
            .expect("handle surface delivery notification");

        let RemoteJsonRpcEvent::SurfaceDelivery(delivery) =
            events.recv().await.expect("surface delivery")
        else {
            panic!("expected surface delivery");
        };
        assert_eq!(delivery.surface_id, SurfaceId::from("surface-remote"));
        assert_eq!(delivery.envelope.session_seq, Some(5));
        assert!(matches!(
            delivery.envelope.event,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running
            })
        ));
    }

    #[tokio::test]
    async fn remote_json_rpc_client_decodes_lifecycle_notifications() {
        let (outbound_tx, _outbound_rx) = mpsc::channel(8);
        let (_client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

        incoming
            .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
                "session/lifecycle",
                Some(serde_json::json!({
                    "surface_id": "surface-remote",
                    "effect": {
                        "type": "session_ended",
                        "session_id": "sess-ended",
                    },
                })),
            )))
            .await
            .expect("handle lifecycle notification");

        let RemoteJsonRpcEvent::SurfaceLifecycle(delivery) =
            events.recv().await.expect("lifecycle delivery")
        else {
            panic!("expected lifecycle delivery");
        };
        assert_eq!(delivery.surface_id, SurfaceId::from("surface-remote"));
        assert_eq!(
            delivery.effect.kind,
            SurfaceLifecycleEffectKind::SessionEnded {
                session_id: test_session_id("sess-ended")
            }
        );
    }

    #[tokio::test]
    async fn remote_json_rpc_client_surfaces_server_requests_and_sends_replies() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);

        incoming
            .handle_frame(JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::String("server-req-1".to_string()),
                "input/requestUserInput",
                Some(serde_json::json!({ "prompt": "continue?" })),
            )))
            .await
            .expect("handle server request");

        let RemoteJsonRpcEvent::ServerRequest(request) =
            events.recv().await.expect("server request event")
        else {
            panic!("expected server request event");
        };
        assert_eq!(request.method, "input/requestUserInput");

        client
            .reply_server_request_success(request.id.clone(), serde_json::json!({ "ok": true }))
            .await
            .expect("send success reply");
        let JsonRpcFrame::Success(success) = outbound_rx.recv().await.expect("success reply")
        else {
            panic!("expected success reply");
        };
        assert_eq!(success.id, request.id);
        assert_eq!(success.result, serde_json::json!({ "ok": true }));

        client
            .reply_server_request_error(
                JsonRpcId::String("server-req-2".to_string()),
                -32603,
                "failed",
                None,
            )
            .await
            .expect("send error reply");
        let JsonRpcFrame::Error(error) = outbound_rx.recv().await.expect("error reply") else {
            panic!("expected error reply");
        };
        assert_eq!(error.id, JsonRpcId::String("server-req-2".to_string()));
        assert_eq!(error.error.code, -32603);
        assert_eq!(error.error.message, "failed");
    }

    #[tokio::test]
    async fn remote_json_rpc_disconnect_resolves_pending_and_invalidates_client() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (client, incoming, mut events) = RemoteJsonRpcClient::new(outbound_tx);
        let request_client = client.clone();

        let request_task = tokio::spawn(async move { request_client.request("slow", None).await });
        let JsonRpcFrame::Request(_request) = outbound_rx.recv().await.expect("outbound request")
        else {
            panic!("expected request frame");
        };

        incoming.disconnect().await;

        assert!(matches!(
            request_task.await.expect("request task"),
            Err(ClientError::Disconnected)
        ));
        assert!(matches!(
            events.recv().await.expect("disconnect event"),
            RemoteJsonRpcEvent::Disconnected
        ));
        assert!(matches!(
            client.request("after/disconnect", None).await,
            Err(ClientError::ClientInvalid)
        ));
    }

    #[tokio::test]
    async fn remote_ndjson_connection_drives_request_response_and_disconnect() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = split(client_stream);
        let (server_read, server_write) = split(server_stream);
        let client_transport =
            NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
        let mut server_transport =
            NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
        let (client, connection, mut events) =
            RemoteJsonRpcClient::connect_ndjson(client_transport);
        let connection_task = tokio::spawn(connection.run());
        let request_client = client.clone();

        let request_task =
            tokio::spawn(async move { request_client.request("control/keepAlive", None).await });
        let Some(JsonRpcFrame::Request(request)) = server_transport
            .recv_frame()
            .await
            .expect("server reads request")
        else {
            panic!("expected request frame");
        };
        server_transport
            .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "ok": true }),
            )))
            .await
            .expect("server writes response");

        assert_eq!(
            request_task
                .await
                .expect("request task")
                .expect("request success"),
            serde_json::json!({ "ok": true })
        );
        drop(server_transport);

        connection_task
            .await
            .expect("connection task")
            .expect("connection exits cleanly");
        assert!(matches!(
            events.recv().await.expect("disconnect event"),
            RemoteJsonRpcEvent::Disconnected
        ));
        assert!(matches!(
            client.request("after/disconnect", None).await,
            Err(ClientError::ClientInvalid)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_json_rpc_client_connects_over_unix_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
            .expect("bind unix listener");
        let server_task =
            tokio::spawn(async move { listener.accept().await.expect("accept unix stream") });

        let (client, connection, mut events) = RemoteJsonRpcClient::connect_unix_with_options(
            &socket_path,
            RemoteConnectOptions {
                outbound_channel_capacity: 8,
                event_channel_capacity: 8,
            },
        )
        .await
        .expect("connect unix socket");
        let mut server_transport = server_task.await.expect("server task");
        let connection_task = tokio::spawn(connection.run());
        let request_client = client.clone();

        let request_task =
            tokio::spawn(async move { request_client.request("control/keepAlive", None).await });
        let Some(JsonRpcFrame::Request(request)) = server_transport
            .recv_frame()
            .await
            .expect("server reads request")
        else {
            panic!("expected request frame");
        };
        server_transport
            .send_frame(&JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "ok": true }),
            )))
            .await
            .expect("server writes response");

        assert_eq!(
            request_task
                .await
                .expect("request task")
                .expect("request success"),
            serde_json::json!({ "ok": true })
        );
        drop(server_transport);

        connection_task
            .await
            .expect("connection task")
            .expect("connection exits cleanly");
        assert!(matches!(
            events.recv().await.expect("disconnect event"),
            RemoteJsonRpcEvent::Disconnected
        ));
        assert!(matches!(
            client.request("after/disconnect", None).await,
            Err(ClientError::ClientInvalid)
        ));
    }

    #[test]
    fn remote_event_demux_buffers_mixed_events_by_surface() {
        let (events_tx, events_rx) = mpsc::channel(8);
        let mut demux = RemoteEventDemux::new(events_rx);
        let first = SurfaceId::from("surface-first");
        let second = SurfaceId::from("surface-second");
        let first_session = test_session_id("sess-first");
        let second_session = test_session_id("sess-second");

        events_tx
            .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
                SurfaceDelivery {
                    surface_id: second.clone(),
                    envelope: durable_envelope(second_session.clone(), 2),
                },
            )))
            .expect("send second event");
        events_tx
            .try_send(RemoteJsonRpcEvent::ServerRequest(JsonRpcRequest::new(
                JsonRpcId::String("server-req".to_string()),
                "input/requestUserInput",
                Some(serde_json::json!({ "prompt": "continue?" })),
            )))
            .expect("send server request");
        events_tx
            .try_send(RemoteJsonRpcEvent::Notification(JsonRpcNotification::new(
                "custom/notice",
                Some(serde_json::json!({ "ok": true })),
            )))
            .expect("send notification");
        events_tx
            .try_send(RemoteJsonRpcEvent::SurfaceLifecycle(
                SurfaceLifecycleDelivery {
                    surface_id: second.clone(),
                    effect: SurfaceLifecycleEffect {
                        surface_id: second.clone(),
                        kind: SurfaceLifecycleEffectKind::SessionEnded {
                            session_id: second_session.clone(),
                        },
                    },
                },
            ))
            .expect("send lifecycle");
        events_tx
            .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
                SurfaceDelivery {
                    surface_id: first.clone(),
                    envelope: durable_envelope(first_session.clone(), 1),
                },
            )))
            .expect("send first event");
        events_tx
            .try_send(RemoteJsonRpcEvent::Disconnected)
            .expect("send disconnect");

        let first_event = demux
            .try_next_surface_event(&first)
            .expect("first surface event");
        assert_eq!(first_event.session_id, first_session);

        let second_event = demux
            .try_next_surface_event(&second)
            .expect("second surface event");
        assert_eq!(second_event.session_id, second_session);

        let server_request = demux
            .try_next_server_request()
            .expect("server request was buffered");
        assert_eq!(server_request.method, "input/requestUserInput");

        let notification = demux
            .try_next_notification()
            .expect("notification was buffered");
        assert_eq!(notification.method, "custom/notice");

        let lifecycle = demux
            .try_next_lifecycle(&second)
            .expect("lifecycle was buffered");
        assert_eq!(
            lifecycle.effect.kind,
            SurfaceLifecycleEffectKind::SessionEnded {
                session_id: second_session
            }
        );

        assert!(demux.try_next_surface_event(&first).is_none());
        assert!(demux.is_disconnected());
    }

    #[test]
    fn remote_surface_stream_reads_events_and_lifecycle_for_one_surface() {
        let (events_tx, events_rx) = mpsc::channel(8);
        let mut demux = RemoteEventDemux::new(events_rx);
        let surface = SurfaceId::from("surface-stream");
        let session = test_session_id("sess-stream");

        events_tx
            .try_send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
                SurfaceDelivery {
                    surface_id: surface.clone(),
                    envelope: durable_envelope(session.clone(), 1),
                },
            )))
            .expect("send surface event");
        events_tx
            .try_send(RemoteJsonRpcEvent::SurfaceLifecycle(
                SurfaceLifecycleDelivery {
                    surface_id: surface.clone(),
                    effect: SurfaceLifecycleEffect {
                        surface_id: surface.clone(),
                        kind: SurfaceLifecycleEffectKind::SessionEnded {
                            session_id: session.clone(),
                        },
                    },
                },
            ))
            .expect("send lifecycle");

        let mut stream = demux.surface_stream(surface.clone());
        assert_eq!(stream.surface_id(), &surface);
        assert_eq!(
            stream.try_next_event().expect("surface event").session_id,
            session
        );
        assert_eq!(
            stream.try_next_lifecycle().expect("lifecycle").effect.kind,
            SurfaceLifecycleEffectKind::SessionEnded {
                session_id: test_session_id("sess-stream")
            }
        );
    }

    #[tokio::test]
    async fn remote_event_demux_async_methods_wait_and_buffer_mixed_events() {
        let (events_tx, events_rx) = mpsc::channel(8);
        let mut demux = RemoteEventDemux::new(events_rx);
        let first = SurfaceId::from("surface-first");
        let second = SurfaceId::from("surface-second");
        let first_session = test_session_id("sess-first");
        let second_session = test_session_id("sess-second");

        events_tx
            .send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
                SurfaceDelivery {
                    surface_id: second.clone(),
                    envelope: durable_envelope(second_session.clone(), 2),
                },
            )))
            .await
            .expect("send second event");
        events_tx
            .send(RemoteJsonRpcEvent::ServerRequest(JsonRpcRequest::new(
                JsonRpcId::String("server-req".to_string()),
                "input/requestUserInput",
                Some(serde_json::json!({ "prompt": "continue?" })),
            )))
            .await
            .expect("send server request");
        events_tx
            .send(RemoteJsonRpcEvent::Notification(JsonRpcNotification::new(
                "custom/notice",
                Some(serde_json::json!({ "ok": true })),
            )))
            .await
            .expect("send notification");
        events_tx
            .send(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(
                SurfaceDelivery {
                    surface_id: first.clone(),
                    envelope: durable_envelope(first_session.clone(), 1),
                },
            )))
            .await
            .expect("send first event");
        events_tx
            .send(RemoteJsonRpcEvent::Disconnected)
            .await
            .expect("send disconnect");

        let first_event = demux.next_surface_event(&first).await.expect("first event");
        assert_eq!(first_event.session_id, first_session);
        let second_event = demux
            .next_surface_event(&second)
            .await
            .expect("second event");
        assert_eq!(second_event.session_id, second_session);
        let server_request = demux.next_server_request().await.expect("server request");
        assert_eq!(server_request.method, "input/requestUserInput");
        let notification = demux.next_notification().await.expect("notification");
        assert_eq!(notification.method, "custom/notice");
        assert!(demux.next_surface_event(&first).await.is_none());
        assert!(demux.is_disconnected());
    }

    #[test]
    fn local_server_client_attaches_interactive_and_passive_surfaces() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        server.route_envelope(durable_envelope(session_id.clone(), 1));
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);

        let interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        assert_eq!(interactive.session_id(), &session_id);
        assert_eq!(passive.session_id(), &session_id);
        assert_eq!(passive.replayed().len(), 1);
        assert_eq!(
            server.list_live_sessions()[0].surface_counts,
            SessionSurfaceCounts {
                attached: 2,
                closed: 0,
            }
        );
        let outcome = server.route_envelope(durable_envelope(session_id, 2));
        assert_eq!(outcome.delivered, 2);
        assert_eq!(
            client
                .events_mut()
                .try_recv()
                .expect("first surface event")
                .envelope
                .session_seq,
            Some(2)
        );
        assert_eq!(
            client
                .events_mut()
                .try_recv()
                .expect("second surface event")
                .envelope
                .session_seq,
            Some(2)
        );
    }

    #[tokio::test]
    async fn local_server_client_next_event_buffers_other_surfaces() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let first_session = test_session_id("sess-1");
        let second_session = test_session_id("sess-2");
        for session_id in [&first_session, &second_session] {
            server
                .registry()
                .begin_load(session_id.clone())
                .expect("reserve session");
            server
                .registry()
                .complete_load_success(session_id, TestHandle("handle"))
                .expect("session live");
        }
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let first = client
            .subscribe_session(
                first_session.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe first");
        let second = client
            .subscribe_session(
                second_session.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe second");

        server.route_envelope(durable_envelope(second_session.clone(), 1));
        server.route_envelope(durable_envelope(first_session.clone(), 1));

        let first_event = client
            .next_passive_event(&first)
            .await
            .expect("first event");
        assert_eq!(first_event.session_id, first_session);
        let buffered_second = client
            .try_next_passive_event(&second)
            .expect("buffered second event");
        assert_eq!(buffered_second.session_id, second_session);
    }

    #[test]
    fn detach_passive_consumes_only_that_surface() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let _interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id, Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        let detached = client.detach_passive(passive).expect("detach passive");

        assert!(detached.detached_surface.is_some());
        assert_eq!(server.list_live_sessions()[0].surface_counts.attached, 1);
    }

    #[test]
    fn client_lists_live_sessions_with_surface_counts() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let session_id = test_session_id("sess-1");
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, TestHandle("handle"))
            .expect("session live");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let client = ServerClient::connect_local(&adapter);
        let _interactive = client
            .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
            .expect("attach interactive");
        let passive = client
            .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe passive");

        assert_eq!(
            client.list_live_sessions(),
            vec![LiveSessionSummary {
                session_id: session_id.clone(),
                surface_counts: SessionSurfaceCounts {
                    attached: 2,
                    closed: 0,
                },
            }]
        );

        client.detach_passive(passive).expect("detach passive");

        assert_eq!(
            client.list_live_sessions(),
            vec![LiveSessionSummary {
                session_id,
                surface_counts: SessionSurfaceCounts {
                    attached: 1,
                    closed: 0,
                },
            }]
        );
    }

    #[test]
    fn session_event_demux_buffers_other_surfaces_on_same_connection() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let interactive_session_id = test_session_id("sess-interactive");
        let passive_session_id = test_session_id("sess-passive");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let interactive = client
            .attach_interactive_session(
                interactive_session_id.clone(),
                AttachSurfaceOptions::default(),
            )
            .expect("attach interactive");
        let passive = client
            .subscribe_session(
                passive_session_id.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe passive");

        server.route_envelope(durable_envelope(passive_session_id.clone(), 1));
        server.route_envelope(durable_envelope(interactive_session_id.clone(), 1));

        let interactive_event = client
            .try_next_session_event(&interactive)
            .expect("interactive event");
        let passive_event = client
            .try_next_passive_event(&passive)
            .expect("passive event");

        assert_eq!(interactive_event.session_id, interactive_session_id);
        assert_eq!(passive_event.session_id, passive_session_id);
        assert!(client.try_next_session_event(&interactive).is_none());
        assert!(client.try_next_passive_event(&passive).is_none());
    }

    #[test]
    fn session_request_demux_buffers_other_interactive_surfaces() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let first_session_id = test_session_id("sess-first");
        let second_session_id = test_session_id("sess-second");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let first = client
            .attach_interactive_session(
                first_session_id.clone(),
                AttachSurfaceOptions {
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach first interactive");
        let second = client
            .attach_interactive_session(
                second_session_id.clone(),
                AttachSurfaceOptions {
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach second interactive");

        let first_route = server
            .route_server_request(
                first_session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-first")),
                test_server_request("first"),
            )
            .expect("route first request");
        let second_route = server
            .route_server_request(
                second_session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-second")),
                test_server_request("second"),
            )
            .expect("route second request");

        let second_delivery = client
            .try_next_session_request(&second)
            .expect("second request");
        let first_delivery = client
            .try_next_session_request(&first)
            .expect("first request");

        assert_eq!(second_delivery.request_id, second_route.pending.request_id);
        assert_eq!(first_delivery.request_id, first_route.pending.request_id);
        assert!(client.try_next_session_request(&first).is_none());
        assert!(client.try_next_session_request(&second).is_none());
    }

    #[test]
    fn lifecycle_demux_buffers_other_surfaces_on_same_connection() {
        let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
        let interactive_session_id = test_session_id("sess-interactive");
        let passive_session_id = test_session_id("sess-passive");
        let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut client = ServerClient::connect_local(&adapter);
        let interactive = client
            .attach_interactive_session(
                interactive_session_id.clone(),
                AttachSurfaceOptions::default(),
            )
            .expect("attach interactive");
        let passive = client
            .subscribe_session(
                passive_session_id.clone(),
                Some(0),
                AttachSurfaceOptions::default(),
            )
            .expect("subscribe passive");

        let outcome = server.route_lifecycle_effects(vec![
            SurfaceLifecycleEffect {
                surface_id: passive.surface_id().clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: passive_session_id,
                },
            },
            SurfaceLifecycleEffect {
                surface_id: interactive.surface_id().clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: interactive_session_id,
                },
            },
        ]);
        assert_eq!(outcome.delivered, 2);

        let interactive_delivery = client
            .try_next_session_lifecycle(&interactive)
            .expect("interactive lifecycle");
        let passive_delivery = client
            .try_next_passive_lifecycle(&passive)
            .expect("passive lifecycle");

        assert_eq!(
            interactive_delivery.surface_id,
            interactive.surface_id().clone()
        );
        assert_eq!(passive_delivery.surface_id, passive.surface_id().clone());
        assert!(client.try_next_session_lifecycle(&interactive).is_none());
        assert!(client.try_next_passive_lifecycle(&passive).is_none());
    }
}
