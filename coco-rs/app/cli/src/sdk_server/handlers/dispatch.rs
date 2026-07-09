use std::path::PathBuf;
use std::sync::Arc;

use coco_types::ClientRequest;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SdkServerState;
use super::config;
use super::mcp;
use super::rewind;
use super::runtime;
use super::session;
use super::turn;
use crate::sdk_server::outbound::OutboundMessage;

/// Per-request context passed to handlers.
pub struct HandlerContext {
    /// Channel for forwarding CoreEvent notifications to the transport.
    /// Handlers that spawn a QueryEngine pass this as the engine's
    /// `event_tx`. Single-shot handlers (e.g., `initialize`) rarely use
    /// it; long-running handlers (e.g., `turn/start`) emit events here.
    pub notif_tx: mpsc::Sender<OutboundMessage>,

    /// Shared server state across requests.
    pub state: Arc<SdkServerState>,

    /// AppServer-derived session scope for the request connection.
    ///
    /// Set only when the connection has exactly one attached interactive
    /// surface. Handlers fall back to the installed runtime's scoped state,
    /// then to a sole keyed handoff when this is absent.
    pub scoped_session_id: Option<coco_types::SessionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveSessionSource {
    Scoped,
    Runtime,
    ScopedState,
}

impl HandlerContext {
    pub fn has_scoped_session(&self) -> bool {
        self.scoped_session_id.is_some()
    }

    pub(super) async fn active_session_resolution(
        &self,
    ) -> Option<(coco_types::SessionId, ActiveSessionSource)> {
        if let Some(session_id) = &self.scoped_session_id {
            return Some((session_id.clone(), ActiveSessionSource::Scoped));
        }
        let runtime = self.state.session_runtime_snapshot().await;
        if let Some(runtime) = runtime {
            let session_id = runtime.current_typed_session_id().await;
            if self.state.session_handoff_snapshot(&session_id).is_some() {
                return Some((session_id, ActiveSessionSource::Runtime));
            }
        }
        if let Some(session_id) = self.state.sole_session_handoff_id() {
            return Some((session_id, ActiveSessionSource::ScopedState));
        }
        None
    }

    pub async fn active_session_id(&self) -> Option<coco_types::SessionId> {
        self.active_session_resolution()
            .await
            .map(|(session_id, _)| session_id)
    }

    pub(super) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        if let Some(session_id) = &self.scoped_session_id
            && let Some(metadata) = self.state.session_metadata_snapshot(session_id)
        {
            return Ok(PathBuf::from(metadata.cwd));
        }
        self.state.workspace_cwd().await
    }
}

/// Result of dispatching a ClientRequest.
pub enum HandlerResult {
    /// Handler succeeded — carries the response `result` payload.
    Ok(Value),
    /// Handler failed with a JSON-RPC error.
    Err {
        code: i32,
        message: String,
        data: Option<Value>,
    },
    /// Handler is not implemented in the current phase. The dispatcher
    /// converts this to a `JsonRpcError` with `METHOD_NOT_FOUND`.
    NotImplemented(String),
}

impl HandlerResult {
    /// Shorthand for a successful empty response.
    pub fn ok_empty() -> Self {
        Self::Ok(Value::Null)
    }

    /// Build an Ok result from any serializable payload. Handler errors
    /// on serialization failure (rare in practice).
    pub fn ok<T: serde::Serialize>(value: T) -> Self {
        match serde_json::to_value(value) {
            Ok(v) => Self::Ok(v),
            Err(e) => Self::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("result serialization failed: {e}"),
                data: None,
            },
        }
    }
}

/// Route a `ClientRequest` to its handler and return the result.
/// The dispatch is exhaustive — adding a new variant to `ClientRequest`
/// fails compilation here, enforcing that every method has a handler.
pub async fn dispatch_client_request(req: ClientRequest, ctx: HandlerContext) -> HandlerResult {
    match req {
        // === Session lifecycle ===
        ClientRequest::Initialize(params) => session::handle_initialize(params, &ctx).await,
        ClientRequest::SessionStart(params) => session::handle_session_start(*params, &ctx).await,
        ClientRequest::SessionResume(params) => session::handle_session_resume(params, &ctx).await,
        ClientRequest::SessionList => session::handle_session_list(&ctx).await,
        ClientRequest::SessionRead(params) => session::handle_session_read(params, &ctx).await,
        ClientRequest::SessionTurnsList(params) => {
            session::handle_session_turns_list(params, &ctx).await
        }
        ClientRequest::SessionSubscribe(_) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/subscribe requires AppServer routing".to_string(),
            data: None,
        },
        ClientRequest::SessionArchive(params) => {
            session::handle_session_archive(params, &ctx).await
        }
        ClientRequest::SessionRename(params) => session::handle_session_rename(params, &ctx).await,
        ClientRequest::SessionToggleTag(params) => {
            session::handle_session_toggle_tag(params, &ctx).await
        }
        ClientRequest::SessionCost => runtime::handle_session_cost(&ctx).await,
        ClientRequest::SessionStatus => runtime::handle_session_status(&ctx).await,

        // === Turn control ===
        ClientRequest::TurnStart(params) => turn::handle_turn_start(params, &ctx).await,
        ClientRequest::TurnInterrupt => turn::handle_turn_interrupt(&ctx).await,

        // === Running task observability ===
        ClientRequest::TaskList => runtime::handle_task_list(&ctx).await,
        ClientRequest::TaskDetail(params) => runtime::handle_task_detail(params, &ctx).await,

        // === Approval + user input + elicitation ===
        ClientRequest::ApprovalResolve(params) => turn::handle_approval_resolve(params, &ctx).await,
        ClientRequest::UserInputResolve(params) => {
            turn::handle_user_input_resolve(params, &ctx).await
        }
        ClientRequest::ElicitationResolve(params) => {
            turn::handle_elicitation_resolve(params, &ctx).await
        }

        // === Runtime control ===
        ClientRequest::SetModel(params) => runtime::handle_set_model(params, &ctx).await,
        ClientRequest::SetModelRole(params) => runtime::handle_set_model_role(params, &ctx).await,
        ClientRequest::SetPermissionMode(params) => {
            runtime::handle_set_permission_mode(params, &ctx).await
        }
        ClientRequest::SetThinking(params) => runtime::handle_set_thinking(params, &ctx).await,
        ClientRequest::SetAgentColor(params) => runtime::handle_set_agent_color(params, &ctx).await,
        ClientRequest::ApplyPermissionUpdate(params) => {
            runtime::handle_apply_permission_update(params, &ctx).await
        }
        ClientRequest::ResetSessionPermissionRules => {
            runtime::handle_reset_session_permission_rules(&ctx).await
        }
        ClientRequest::StopTask(params) => runtime::handle_stop_task(params, &ctx).await,
        ClientRequest::RewindFiles(params) => rewind::handle_rewind_files(params, &ctx).await,
        ClientRequest::UpdateEnv(params) => runtime::handle_update_env(params, &ctx).await,
        ClientRequest::BackgroundAllTasks => runtime::handle_background_all_tasks(&ctx).await,

        // `keepAlive` is the simplest handler — respond with empty ok so
        // clients using it as a heartbeat get immediate acknowledgement.
        ClientRequest::KeepAlive => HandlerResult::ok_empty(),

        ClientRequest::CancelRequest(params) => turn::handle_cancel_request(params, &ctx).await,
        ClientRequest::AgentInterruptCurrentWork(params) => {
            runtime::handle_agent_interrupt_current_work(params, &ctx).await
        }

        // === Config ===
        ClientRequest::ConfigRead => config::handle_config_read(&ctx).await,
        ClientRequest::ConfigWrite(params) => config::handle_config_write(params, &ctx).await,

        // === TS P1 gap additions ===
        ClientRequest::McpStatus => mcp::handle_mcp_status(&ctx).await,
        ClientRequest::ContextUsage => runtime::handle_context_usage(&ctx).await,
        ClientRequest::McpSetServers(params) => mcp::handle_mcp_set_servers(params, &ctx).await,
        ClientRequest::McpReconnect(params) => mcp::handle_mcp_reconnect(params, &ctx).await,
        ClientRequest::McpToggle(params) => mcp::handle_mcp_toggle(params, &ctx).await,
        ClientRequest::PluginReload => runtime::handle_plugin_reload(&ctx).await,
        ClientRequest::HookReload => runtime::handle_hook_reload(&ctx).await,
        ClientRequest::ConfigApplyFlags(params) => {
            runtime::handle_config_apply_flags(params, &ctx).await
        }
    }
}
