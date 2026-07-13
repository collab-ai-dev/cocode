use std::collections::{HashMap, VecDeque};

use coco_app_server::{
    AttachSurfaceOptions, DetachSurfaceOutcome, DisconnectOutcome, LocalClientAdapter,
    LocalClientConnection, LocalClientDispatchError, LocalClientRequestHandler,
    LocalClientSubscribeOutcome, SessionSurfaceCounts, SurfaceRole,
};
use coco_types::{
    AgentInterruptCurrentWorkParams, ApplyPermissionUpdateParams, ApprovalResolveParams,
    BackgroundAllTasksResult, CancelRequestParams, ClientRequest, ConfigApplyFlagsParams,
    ConfigReadParams, ConfigReadResult, ConfigWriteParams, ContextUsageResult,
    ElicitationResolveParams, HookReloadResult, InitializeParams, InitializeResult,
    InteractiveTarget, McpReconnectParams, McpSetServersParams, McpSetServersResult,
    McpStatusResult, McpToggleParams, PluginReloadResult, ResetSessionPermissionRulesResult,
    RewindFilesParams, RewindFilesResult, ServerRequestDelivery, SessionCloseParams,
    SessionCloseTarget, SessionCostResult, SessionDeleteParams, SessionEnvelope, SessionId,
    SessionListResult, SessionReadParams, SessionReadResult, SessionRenameParams,
    SessionRenameResult, SessionResumeParams, SessionResumeResult, SessionStartParams,
    SessionStartResult, SessionStatusResult, SessionSubscribeParams, SessionSubscribeResult,
    SessionTarget, SessionToggleTagParams, SessionToggleTagResult, SessionTurnsListParams,
    SessionTurnsListResult, SetAgentColorParams, SetModelParams, SetModelRoleParams,
    SetModelRoleResult, SetPermissionModeParams, SetThinkingParams, StopTaskParams,
    SurfaceDelivery, SurfaceId, SurfaceLifecycleEffect, TaskDetailParams, TaskDetailResult,
    TaskListResult, TurnStartParams, TurnStartResult, UpdateEnvParams, UserInputResolveParams,
};

use coco_app_server_client::ClientError;

mod controls;
mod data;
mod session_lifecycle;
mod session_ops;
mod surfaces;
mod tasks;
mod turn;

fn dispatch_error(error: LocalClientDispatchError) -> ClientError {
    ClientError::Server {
        code: error.code,
        message: error.message,
        data: error.data,
    }
}

pub(crate) fn client_error_from_attach(error: coco_app_server::AttachError) -> ClientError {
    ClientError::InvalidArgument(error.to_string())
}

pub struct LocalServerClient<H> {
    connection: LocalClientConnection<H>,
    event_buffers: HashMap<SurfaceId, VecDeque<SessionEnvelope>>,
    request_buffers: HashMap<SurfaceId, VecDeque<ServerRequestDelivery>>,
    lifecycle_buffers: HashMap<SurfaceId, VecDeque<SurfaceLifecycleEffect>>,
}

impl<H: Clone> LocalServerClient<H> {
    pub fn connect_local(adapter: &LocalClientAdapter<H>) -> Self {
        Self {
            connection: adapter.connect(),
            event_buffers: HashMap::new(),
            request_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
        }
    }

    pub fn disconnect(self) -> coco_app_server::DisconnectOutcome {
        self.connection.disconnect()
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

    /// Drop this surface's buffered events/requests/lifecycle after it is
    /// detached or its session closed.
    fn purge_surface_buffers(&mut self, surface_id: &SurfaceId) {
        self.event_buffers.remove(surface_id);
        self.request_buffers.remove(surface_id);
        self.lifecycle_buffers.remove(surface_id);
    }

    pub fn close(self) -> Result<DisconnectOutcome, ClientError> {
        Ok(self.connection.disconnect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
}

impl LocalSessionClient {
    pub fn session_target(&self) -> SessionTarget {
        SessionTarget {
            session_id: self.session_id.clone(),
        }
    }

    pub fn interactive_target(&self) -> InteractiveTarget {
        InteractiveTarget {
            session_id: self.session_id.clone(),
            surface_id: self.surface_id.clone(),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    /// Mint the successor handle after a server-committed replace re-points this
    /// surface to `new_session_id`. Consumes `self`: the
    /// identity rule is that a handle is never re-pointed in place.
    pub fn into_replaced(self, new_session_id: SessionId) -> LocalSessionClient {
        LocalSessionClient {
            session_id: new_session_id,
            surface_id: self.surface_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalPassiveSessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
    replayed: Vec<SessionEnvelope>,
}

impl LocalPassiveSessionClient {
    fn session_target(&self) -> SessionTarget {
        SessionTarget {
            session_id: self.session_id.clone(),
        }
    }

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
pub struct LocalLiveSessionSummary {
    pub session_id: SessionId,
    pub surface_counts: SessionSurfaceCounts,
}

#[cfg(test)]
#[path = "local_client.test.rs"]
mod tests;
