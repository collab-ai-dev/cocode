use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use coco_app_server::{
    AttachSessionOptions, DetachSessionOutcome, DisconnectOutcome, LocalClientAdapter,
    LocalClientDispatchError, LocalClientHandle, LocalClientRequestHandler,
    LocalClientSubscribeOutcome,
};
use coco_types::{
    AgentInterruptCurrentWorkParams, ApplyPermissionUpdateParams, ApprovalResolveParams,
    BackgroundAllTasksResult, CancelRequestParams, ClientRequest, ConfigApplyFlagsParams,
    ConfigReadParams, ConfigReadResult, ConfigWriteParams, ContextUsageResult,
    ElicitationResolveParams, HookReloadResult, InitializeParams, InitializeResult,
    McpReconnectParams, McpSetServersParams, McpSetServersResult, McpStatusResult, McpToggleParams,
    PluginReloadResult, ResetSessionPermissionRulesResult, RewindFilesParams, RewindFilesResult,
    SessionCloseParams, SessionCostResult, SessionDeleteParams, SessionEnvelope, SessionId,
    SessionListResult, SessionReadParams, SessionReadResult, SessionRenameParams,
    SessionRenameResult, SessionResumeParams, SessionResumeResult, SessionStartParams,
    SessionStartResult, SessionStatusResult, SessionSubscribeParams, SessionSubscribeResult,
    SessionTarget, SessionToggleTagParams, SessionToggleTagResult, SessionTurnsListParams,
    SessionTurnsListResult, SetAgentColorParams, SetModelParams, SetModelRoleParams,
    SetModelRoleResult, SetPermissionModeParams, SetThinkingParams, StopTaskParams,
    TaskDetailParams, TaskDetailResult, TaskListResult, TurnStartParams, TurnStartResult,
    UpdateEnvParams, UserInputResolveParams,
};

use coco_app_server_client::ClientError;

mod controls;
mod data;
mod session_lifecycle;
mod session_ops;
mod sessions;
mod tasks;
mod turn;

fn dispatch_error(error: LocalClientDispatchError) -> ClientError {
    ClientError::Server {
        code: error.code,
        message: error.message,
        data: error.data,
    }
}

pub struct LocalServerClient<H> {
    handle: LocalClientHandle<H>,
    inbound_owner: Arc<LocalInboundOwner>,
    events: tokio::sync::broadcast::Receiver<SessionEnvelope>,
}

struct LocalInboundOwner {
    state: Arc<Mutex<LocalInboundState>>,
    notify: Arc<tokio::sync::Notify>,
    events: tokio::sync::broadcast::Sender<SessionEnvelope>,
    task: tokio::task::JoinHandle<()>,
}

struct LocalInboundState {
    capacity: usize,
    requests: HashMap<SessionId, VecDeque<coco_types::ServerRequestDelivery>>,
    request_count: usize,
    lifecycle: VecDeque<coco_types::SessionLifecycleEffect>,
    dropped_lifecycle: u64,
    closed: bool,
}

impl LocalInboundState {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            requests: HashMap::new(),
            request_count: 0,
            lifecycle: VecDeque::new(),
            dropped_lifecycle: 0,
            closed: false,
        }
    }

    fn push(&mut self, inbound: coco_app_server::LocalClientInbound) -> Result<(), ()> {
        match inbound {
            coco_app_server::LocalClientInbound::Event(_) => {
                unreachable!("session events use the per-view broadcast channel")
            }
            coco_app_server::LocalClientInbound::ServerRequest(delivery) => {
                if self.request_count == self.capacity {
                    return Err(());
                }
                let delivery = *delivery;
                self.requests
                    .entry(delivery.session_id.clone())
                    .or_default()
                    .push_back(delivery);
                self.request_count += 1;
            }
            coco_app_server::LocalClientInbound::Lifecycle(effect) => {
                // Lifecycle effects are observational; a plain (non-sidechat)
                // TUI session has no continuous lifecycle consumer, so a full
                // queue drops the oldest effect instead of killing the
                // connection — overflow here must never silence the client.
                if self.lifecycle.len() == self.capacity {
                    self.lifecycle.pop_front();
                    self.dropped_lifecycle = self.dropped_lifecycle.saturating_add(1);
                    tracing::warn!(
                        dropped_total = self.dropped_lifecycle,
                        capacity = self.capacity,
                        "local client lifecycle queue full; dropping oldest effect"
                    );
                }
                self.lifecycle.push_back(effect);
            }
        }
        Ok(())
    }
}

impl Drop for LocalInboundOwner {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl<H> Clone for LocalServerClient<H> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            inbound_owner: Arc::clone(&self.inbound_owner),
            events: self.inbound_owner.events.subscribe(),
        }
    }
}

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    pub fn connect_local(adapter: &LocalClientAdapter<H>) -> Self {
        let mut connection = adapter.connect();
        let handle = connection.handle();
        let state = Arc::new(Mutex::new(LocalInboundState::new(
            adapter.channel_capacity().saturating_mul(4).max(64),
        )));
        let owner_state = Arc::clone(&state);
        let notify = Arc::new(tokio::sync::Notify::new());
        let owner_notify = Arc::clone(&notify);
        let event_capacity = adapter.channel_capacity().saturating_mul(4).max(64);
        let (events, event_receiver) = tokio::sync::broadcast::channel(event_capacity);
        let owner_events = events.clone();
        let task = tokio::spawn(async move {
            while let Some(inbound) = connection.recv().await {
                let inbound = match inbound {
                    coco_app_server::LocalClientInbound::Event(delivery) => {
                        let _ = owner_events.send(delivery.envelope);
                        continue;
                    }
                    other => other,
                };
                let mut state = owner_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.push(inbound).is_err() {
                    state.closed = true;
                    let pending_requests = state.request_count;
                    drop(state);
                    // Never die silently: this ends every observer on the
                    // TUI's only physical connection.
                    tracing::error!(
                        pending_requests,
                        "local client server-request queue overflowed; closing the in-process client"
                    );
                    owner_notify.notify_waiters();
                    return;
                }
                drop(state);
                owner_notify.notify_waiters();
            }
            owner_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .closed = true;
            owner_notify.notify_waiters();
        });
        Self {
            handle,
            inbound_owner: Arc::new(LocalInboundOwner {
                state,
                notify,
                events,
                task,
            }),
            events: event_receiver,
        }
    }

    pub fn connection_key(&self) -> coco_app_server::ConnectionKey {
        self.handle.connection_key()
    }

    pub async fn send_client_request<Handler>(
        &self,
        handler: &Handler,
        request: ClientRequest,
    ) -> Result<serde_json::Value, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.handle
            .dispatch_client_request(handler, request)
            .await
            .map_err(dispatch_error)
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

    pub fn close(&self) -> DisconnectOutcome {
        self.handle.disconnect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSessionClient {
    session_id: SessionId,
}

impl LocalSessionClient {
    pub fn session_target(&self) -> SessionTarget {
        SessionTarget {
            session_id: self.session_id.clone(),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

/// Read-only session view produced by [`LocalServerClient::subscribe_session`].
#[derive(Debug, Clone)]
pub struct LocalReadOnlySessionClient {
    session_id: SessionId,
    replayed: Vec<SessionEnvelope>,
}

impl LocalReadOnlySessionClient {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn replayed(&self) -> &[SessionEnvelope] {
        &self.replayed
    }
}

#[cfg(test)]
#[path = "local_client.test.rs"]
mod tests;
