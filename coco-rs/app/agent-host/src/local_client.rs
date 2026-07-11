use std::collections::HashMap;
use std::collections::VecDeque;

use coco_app_server::AttachSurfaceOptions;
use coco_app_server::DetachSurfaceOutcome;
use coco_app_server::DisconnectOutcome;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientConnection;
use coco_app_server::LocalClientDispatchError;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server::LocalClientSubscribeOutcome;
use coco_app_server::SessionSurfaceCounts;
use coco_app_server::SurfaceRole;
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
use coco_types::ServerRequestDelivery;
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
use coco_types::SurfaceDelivery;
use coco_types::SurfaceId;
use coco_types::SurfaceLifecycleEffect;
use coco_types::TaskDetailParams;
use coco_types::TaskDetailResult;
use coco_types::TaskListResult;
use coco_types::TurnStartParams;
use coco_types::TurnStartResult;
use coco_types::UpdateEnvParams;
use coco_types::UserInputResolveParams;

use coco_app_server_client::ClientError;

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

    pub async fn session_start_handle<Handler>(
        &self,
        handler: &Handler,
        params: SessionStartParams,
    ) -> Result<LocalSessionClient, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        let started = self.session_start(handler, params).await?;
        let surface_id = match started.surface_id {
            Some(surface_id) => surface_id,
            None => {
                return self.attach_interactive_session(
                    started.session_id,
                    AttachSurfaceOptions::default(),
                );
            }
        };
        Ok(LocalSessionClient {
            session_id: started.session_id,
            surface_id,
        })
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

    pub async fn session_resume_handle<Handler>(
        &self,
        handler: &Handler,
        params: SessionResumeParams,
    ) -> Result<LocalSessionClient, ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        let resumed = self.session_resume(handler, params).await?;
        let session_id = resumed.session.session_id;
        let surface_id = match resumed.surface_id {
            Some(surface_id) => surface_id,
            None => {
                return self
                    .attach_interactive_session(session_id, AttachSurfaceOptions::default());
            }
        };
        Ok(LocalSessionClient {
            session_id,
            surface_id,
        })
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
        _session: &LocalSessionClient,
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
        _session: &LocalSessionClient,
    ) -> Result<(), ClientError>
    where
        Handler: LocalClientRequestHandler,
    {
        self.turn_interrupt(handler).await
    }

    pub async fn close_session<Handler>(
        &mut self,
        handler: &Handler,
        session: LocalSessionClient,
    ) -> Result<(), (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        let params = SessionArchiveParams {
            session_id: session.session_id.clone(),
        };
        match self.session_archive(handler, params).await {
            Ok(()) => {
                self.purge_surface_buffers(&session.surface_id);
                Ok(())
            }
            Err(error) => Err((session, error)),
        }
    }

    pub async fn replace_session_with_start<Handler>(
        &self,
        handler: &Handler,
        session: LocalSessionClient,
        params: SessionStartParams,
    ) -> Result<LocalSessionClient, (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        let old_surface_id = session.surface_id.clone();
        match self.session_start(handler, params).await {
            Ok(started) => Ok(LocalSessionClient {
                session_id: started.session_id,
                surface_id: started.surface_id.unwrap_or(old_surface_id),
            }),
            Err(error) => Err((session, error)),
        }
    }

    pub async fn replace_session_with_resume<Handler>(
        &self,
        handler: &Handler,
        session: LocalSessionClient,
        params: SessionResumeParams,
    ) -> Result<LocalSessionClient, (LocalSessionClient, ClientError)>
    where
        Handler: LocalClientRequestHandler,
    {
        let old_surface_id = session.surface_id.clone();
        match self.session_resume(handler, params).await {
            Ok(resumed) => Ok(LocalSessionClient {
                session_id: resumed.session.session_id,
                surface_id: resumed.surface_id.unwrap_or(old_surface_id),
            }),
            Err(error) => Err((session, error)),
        }
    }

    pub async fn read_passive_session<Handler>(
        &self,
        handler: &Handler,
        session: &LocalPassiveSessionClient,
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
        session: &LocalPassiveSessionClient,
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
    ) -> Result<LocalSessionClient, ClientError> {
        options.role = SurfaceRole::Interactive;
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(client_error_from_attach)?;
        Ok(LocalSessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
        })
    }

    /// Live-only passive attach WITHOUT replay: attaches a passive surface via
    /// `attach_surface` (not `subscribe` with a cursor), so it never returns
    /// `SnapshotRequired`. This is the no-replay tail attach the TUI
    /// turn-completion monitors use instead of `subscribe_session (id, Some (0))`
    ///. The returned handle carries no replayed envelopes.
    pub fn attach_passive_session(
        &self,
        session_id: SessionId,
    ) -> Result<LocalPassiveSessionClient, ClientError> {
        let options = AttachSurfaceOptions {
            role: SurfaceRole::Passive,
            ..AttachSurfaceOptions::default()
        };
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(client_error_from_attach)?;
        Ok(LocalPassiveSessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
            replayed: Vec::new(),
        })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<LocalPassiveSessionClient, ClientError> {
        let subscription = self
            .connection
            .subscribe_surface(session_id, after_seq, options)
            .map_err(client_error_from_attach)?;
        match subscription {
            LocalClientSubscribeOutcome::Attached(subscription) => Ok(LocalPassiveSessionClient {
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

    pub fn try_next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        self.next_event_for_surface(session.surface_id()).await
    }

    pub fn try_next_passive_event(
        &mut self,
        session: &LocalPassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_passive_event(
        &mut self,
        session: &LocalPassiveSessionClient,
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
        session: &LocalSessionClient,
    ) -> Option<ServerRequestDelivery> {
        self.try_next_request_for_surface(session.surface_id())
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleEffect> {
        self.connection.lifecycle_mut()
    }

    pub fn try_next_session_lifecycle(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SurfaceLifecycleEffect> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn try_next_passive_lifecycle(
        &mut self,
        session: &LocalPassiveSessionClient,
    ) -> Option<SurfaceLifecycleEffect> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn detach_passive(
        &mut self,
        passive: LocalPassiveSessionClient,
    ) -> Result<DetachSurfaceOutcome, (LocalPassiveSessionClient, ClientError)> {
        let outcome = self.connection.detach_surface(&passive.surface_id);
        if outcome.detached_surface.is_some() {
            self.purge_surface_buffers(&passive.surface_id);
            Ok(outcome)
        } else {
            Err((
                passive,
                ClientError::InvalidArgument("passive surface is not attached".to_string()),
            ))
        }
    }

    /// Drop this surface's buffered events/requests/lifecycle after it is
    /// detached or its session archived.
    fn purge_surface_buffers(&mut self, surface_id: &SurfaceId) {
        self.event_buffers.remove(surface_id);
        self.request_buffers.remove(surface_id);
        self.lifecycle_buffers.remove(surface_id);
    }

    pub fn close(self) -> Result<DisconnectOutcome, ClientError> {
        Ok(self.connection.disconnect())
    }

    pub fn list_live_sessions(&self) -> Vec<LocalLiveSessionSummary> {
        self.connection
            .list_live_sessions()
            .into_iter()
            .map(|summary| LocalLiveSessionSummary {
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
    ) -> Option<SurfaceLifecycleEffect> {
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
pub struct LocalSessionClient {
    session_id: SessionId,
    surface_id: SurfaceId,
}

impl LocalSessionClient {
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
