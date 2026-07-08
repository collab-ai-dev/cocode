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
use coco_types::AgentId;
use coco_types::AgentInterruptCurrentWorkParams;
use coco_types::AgentStreamEvent;
use coco_types::ApplyPermissionUpdateParams;
use coco_types::ApprovalResolveParams;
use coco_types::BackgroundAllTasksResult;
use coco_types::CancelRequestParams;
use coco_types::ClientRequest;
use coco_types::ConfigApplyFlagsParams;
use coco_types::ConfigReadResult;
use coco_types::ConfigWriteParams;
use coco_types::ContextUsageResult;
use coco_types::CoreEvent;
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
use coco_types::ServerNotification;
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
use coco_types::SessionSubscribeEnvelope;
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
use coco_types::TaskDetailParams;
use coco_types::TaskDetailResult;
use coco_types::TaskListResult;
use coco_types::TuiOnlyEvent;
use coco_types::TurnStartParams;
use coco_types::TurnStartResult;
use coco_types::UpdateEnvParams;
use coco_types::UserInputResolveParams;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

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

    pub async fn session_turns_list<Handler>(
        &self,
        handler: &Handler,
        params: SessionTurnsListParams,
    ) -> Result<SessionTurnsListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionTurnsList(params))
            .await
    }

    pub async fn session_subscribe<Handler>(
        &self,
        handler: &Handler,
        params: SessionSubscribeParams,
    ) -> Result<SessionSubscribeResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionSubscribe(params))
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

    pub async fn session_rename<Handler>(
        &self,
        handler: &Handler,
        params: SessionRenameParams,
    ) -> Result<SessionRenameResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionRename(params))
            .await
    }

    pub async fn session_toggle_tag<Handler>(
        &self,
        handler: &Handler,
        params: SessionToggleTagParams,
    ) -> Result<SessionToggleTagResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionToggleTag(params))
            .await
    }

    pub async fn session_cost<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<SessionCostResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionCost)
            .await
    }

    pub async fn session_status<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<SessionStatusResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SessionStatus)
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

    pub async fn query_session<Handler>(
        &self,
        handler: &Handler,
        _session: &SessionClient,
        params: TurnStartParams,
    ) -> Result<TurnStartResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.turn_start(handler, params).await
    }

    pub async fn interrupt_session<Handler>(
        &self,
        handler: &Handler,
        _session: &SessionClient,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.turn_interrupt(handler).await
    }

    pub async fn close_session<Handler>(
        &self,
        handler: &Handler,
        session: SessionClient,
    ) -> Result<(), (SessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        let params = SessionArchiveParams {
            session_id: session.session_id.clone(),
        };
        match self.session_archive(handler, params).await {
            Ok(()) => Ok(()),
            Err(error) => Err((session, error)),
        }
    }

    pub async fn read_passive_session<Handler>(
        &self,
        handler: &Handler,
        session: &PassiveSessionClient,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionReadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.session_read(
            handler,
            SessionReadParams {
                session_id: session.session_id.clone(),
                cursor,
                limit,
            },
        )
        .await
    }

    pub async fn list_passive_session_turns<Handler>(
        &self,
        handler: &Handler,
        session: &PassiveSessionClient,
        cursor: Option<String>,
        limit: Option<i32>,
    ) -> Result<SessionTurnsListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.session_turns_list(
            handler,
            SessionTurnsListParams {
                session_id: session.session_id.clone(),
                cursor,
                limit,
            },
        )
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

    pub async fn set_model_role<Handler>(
        &self,
        handler: &Handler,
        params: SetModelRoleParams,
    ) -> Result<SetModelRoleResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetModelRole(params))
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

    pub async fn set_agent_color<Handler>(
        &self,
        handler: &Handler,
        params: SetAgentColorParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::SetAgentColor(params))
            .await
    }

    pub async fn apply_permission_update<Handler>(
        &self,
        handler: &Handler,
        params: ApplyPermissionUpdateParams,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ApplyPermissionUpdate(params))
            .await
    }

    pub async fn reset_session_permission_rules<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<ResetSessionPermissionRulesResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::ResetSessionPermissionRules)
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

    pub async fn task_list<Handler>(&self, handler: &Handler) -> Result<TaskListResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TaskList)
            .await
    }

    pub async fn task_detail<Handler>(
        &self,
        handler: &Handler,
        params: TaskDetailParams,
    ) -> Result<TaskDetailResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::TaskDetail(params))
            .await
    }

    pub async fn background_all_tasks<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<BackgroundAllTasksResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::BackgroundAllTasks)
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

    pub async fn hook_reload<Handler>(
        &self,
        handler: &Handler,
    ) -> Result<HookReloadResult, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.send_typed_client_request(handler, ClientRequest::HookReload)
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

    pub fn with_session_id(&self, session_id: SessionId) -> Self {
        Self {
            session_id,
            surface_id: self.surface_id.clone(),
        }
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

pub struct RemoteWebSocketConnection<S> {
    incoming: RemoteJsonRpcIncoming,
    outbound: mpsc::Receiver<JsonRpcFrame>,
    websocket: WebSocketStream<S>,
}

pub type RemoteDefaultWebSocketConnection =
    RemoteWebSocketConnection<MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(unix)]
pub type RemoteNdjsonUnixConnection = RemoteNdjsonConnection<
    tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
>;

#[cfg(windows)]
pub type RemoteNdjsonNamedPipeConnection = RemoteNdjsonConnection<
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
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

pub struct RemoteOwnedSurfaceStream {
    demux: RemoteEventDemux,
    surface_id: SurfaceId,
}

#[derive(Clone)]
pub struct RemoteSessionClient {
    client: RemoteJsonRpcClient,
    session_id: SessionId,
    surface_id: SurfaceId,
}

#[derive(Clone)]
pub struct RemotePassiveSessionClient {
    client: RemoteJsonRpcClient,
    session_id: SessionId,
    surface_id: SurfaceId,
    replayed: Vec<SessionEnvelope>,
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

    pub fn try_next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SurfaceLifecycleDelivery> {
        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
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

    pub async fn next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SurfaceLifecycleDelivery> {
        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
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

    pub fn into_surface_stream(self, surface_id: SurfaceId) -> RemoteOwnedSurfaceStream {
        RemoteOwnedSurfaceStream {
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

impl RemoteOwnedSurfaceStream {
    pub fn new(demux: RemoteEventDemux, surface_id: SurfaceId) -> Self {
        Self { demux, surface_id }
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn demux_mut(&mut self) -> &mut RemoteEventDemux {
        &mut self.demux
    }

    pub fn into_demux(self) -> RemoteEventDemux {
        self.demux
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

    #[cfg(windows)]
    pub async fn connect_named_pipe(
        pipe_name: impl AsRef<str>,
    ) -> Result<
        (
            Self,
            RemoteNdjsonNamedPipeConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        TransportFrameError,
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
        TransportFrameError,
    > {
        Self::connect_named_pipe_with_channel_capacity(
            pipe_name,
            options.outbound_channel_capacity,
            options.event_channel_capacity,
        )
        .await
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
        TransportFrameError,
    > {
        let transport = coco_app_server_transport::connect_ndjson_named_pipe(pipe_name)?;
        Ok(Self::connect_ndjson_with_channel_capacity(
            transport,
            outbound_channel_capacity,
            event_channel_capacity,
        ))
    }

    pub async fn connect_websocket(
        url: &str,
    ) -> Result<
        (
            Self,
            RemoteDefaultWebSocketConnection,
            mpsc::Receiver<RemoteJsonRpcEvent>,
        ),
        tokio_tungstenite::tungstenite::Error,
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
        tokio_tungstenite::tungstenite::Error,
    > {
        Self::connect_websocket_with_channel_capacity(
            url,
            options.outbound_channel_capacity,
            options.event_channel_capacity,
        )
        .await
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
        tokio_tungstenite::tungstenite::Error,
    > {
        let (websocket, _) = connect_async(url).await?;
        assert!(
            outbound_channel_capacity > 0,
            "remote outbound channel capacity must be non-zero"
        );
        let (outbound_tx, outbound_rx) = mpsc::channel(outbound_channel_capacity);
        let (client, incoming, events) =
            Self::with_event_channel_capacity(outbound_tx, event_channel_capacity);
        let connection = RemoteWebSocketConnection {
            incoming,
            outbound: outbound_rx,
            websocket,
        };
        Ok((client, connection, events))
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
    ) -> Option<SurfaceLifecycleDelivery> {
        demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleDelivery> {
        demux.next_lifecycle(&self.surface_id).await
    }

    pub async fn query(&self, params: TurnStartParams) -> Result<TurnStartResult, ClientError> {
        self.client.turn_start(params).await
    }

    pub async fn interrupt(&self) -> Result<(), ClientError> {
        self.client.turn_interrupt().await
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
    ) -> Option<SurfaceLifecycleDelivery> {
        demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(
        &self,
        demux: &mut RemoteEventDemux,
    ) -> Option<SurfaceLifecycleDelivery> {
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
        let error = ClientError::from_json_rpc_error(code, message, data);
        let _ = pending.reply.send(Err(error));
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

impl<S> RemoteWebSocketConnection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn run(self) -> Result<(), RemoteTransportError> {
        let RemoteWebSocketConnection {
            incoming,
            mut outbound,
            mut websocket,
        } = self;
        let result = loop {
            tokio::select! {
                message = websocket.next() => {
                    let Some(message) = message else {
                        break Ok(());
                    };
                    let message = message.map_err(|source| RemoteTransportError::WebSocket { source })?;
                    match remote_json_rpc_frame_from_websocket_message(message)? {
                        RemoteWebSocketInboundFrame::Frame(frame) => incoming.handle_frame(frame).await.map_err(|source| RemoteTransportError::Client { source })?,
                        RemoteWebSocketInboundFrame::Ignore => {}
                        RemoteWebSocketInboundFrame::Closed => break Ok(()),
                    }
                }
                frame = outbound.recv() => {
                    let Some(frame) = frame else {
                        let _ = websocket.close(None).await;
                        break Ok(());
                    };
                    write_remote_websocket_json_rpc_frame(&mut websocket, &frame).await?;
                }
            }
        };
        incoming.disconnect().await;
        result
    }
}

enum RemoteWebSocketInboundFrame {
    Frame(JsonRpcFrame),
    Ignore,
    Closed,
}

fn remote_json_rpc_frame_from_websocket_message(
    message: WebSocketMessage,
) -> Result<RemoteWebSocketInboundFrame, RemoteTransportError> {
    match message {
        WebSocketMessage::Text(text) => serde_json::from_str(text.as_ref())
            .map(RemoteWebSocketInboundFrame::Frame)
            .map_err(|source| RemoteTransportError::DecodeWebSocketFrame { source }),
        WebSocketMessage::Binary(bytes) => serde_json::from_slice(bytes.as_ref())
            .map(RemoteWebSocketInboundFrame::Frame)
            .map_err(|source| RemoteTransportError::DecodeWebSocketFrame { source }),
        WebSocketMessage::Close(_) => Ok(RemoteWebSocketInboundFrame::Closed),
        WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {
            Ok(RemoteWebSocketInboundFrame::Ignore)
        }
        WebSocketMessage::Frame(_) => Ok(RemoteWebSocketInboundFrame::Ignore),
    }
}

async fn write_remote_websocket_json_rpc_frame<S>(
    websocket: &mut WebSocketStream<S>,
    frame: &JsonRpcFrame,
) -> Result<(), RemoteTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame)
        .map_err(|source| RemoteTransportError::EncodeWebSocketFrame { source })?;
    websocket
        .send(WebSocketMessage::Text(text.into()))
        .await
        .map_err(|source| RemoteTransportError::WebSocket { source })
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

fn lifecycle_activates_session(
    delivery: &SurfaceLifecycleDelivery,
    session_id: &SessionId,
) -> bool {
    match &delivery.effect.kind {
        SurfaceLifecycleEffectKind::SessionStarted {
            session_id: started,
        } => started == session_id,
        SurfaceLifecycleEffectKind::SessionReplaced { new_session_id, .. } => {
            new_session_id == session_id
        }
        SurfaceLifecycleEffectKind::SessionEnded { .. } => false,
    }
}

fn domain_error_kind(data: Option<&serde_json::Value>) -> Option<&str> {
    data.and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
}

fn decode_session_subscribe_envelope(
    envelope: SessionSubscribeEnvelope,
) -> Result<SessionEnvelope, ClientError> {
    let event = match envelope.event {
        serde_json::Value::Object(event) => event,
        _ => {
            return Err(ClientError::InvalidArgument(
                "session/subscribe replay event must be an object".to_string(),
            ));
        }
    };
    Ok(SessionEnvelope {
        session_id: envelope.session_id,
        agent_id: envelope
            .agent_id
            .map(AgentId::try_new)
            .transpose()
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid replay agent_id: {error}"))
            })?,
        turn_id: envelope.turn_id,
        session_seq: envelope.session_seq,
        event: decode_core_event(event)?,
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
    WebSocket {
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[error("failed to encode websocket JSON-RPC frame: {source}")]
    EncodeWebSocketFrame { source: serde_json::Error },
    #[error("failed to decode websocket JSON-RPC frame: {source}")]
    DecodeWebSocketFrame { source: serde_json::Error },
    #[error("{source}")]
    Client { source: ClientError },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
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
#[path = "lib.test.rs"]
mod tests;
