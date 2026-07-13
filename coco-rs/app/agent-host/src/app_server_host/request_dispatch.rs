use coco_types::ClientRequest;

use crate::app_server_host::{HandlerContext, HandlerResult};

use super::request_handlers::{config, mcp, rewind, runtime, session, turn};

/// Route a `ClientRequest` to its handler and return the result.
/// The dispatch is exhaustive — adding a new variant to `ClientRequest`
/// fails compilation here, enforcing that every method has a handler.
pub async fn dispatch_client_request(req: ClientRequest, ctx: HandlerContext) -> HandlerResult {
    match req {
        // === Session lifecycle ===
        ClientRequest::Initialize(params) => session::handle_initialize(params, &ctx).await,
        ClientRequest::SessionStart(_) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/start requires AppServer lifecycle routing".to_string(),
            data: Some(serde_json::json!({ "kind": "app_server_required" })),
        },
        ClientRequest::SessionResume(_) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/resume requires AppServer lifecycle routing".to_string(),
            data: Some(serde_json::json!({ "kind": "app_server_required" })),
        },
        ClientRequest::SessionReplace(_) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/replace requires AppServer lifecycle routing".to_string(),
            data: None,
        },
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
        ClientRequest::SessionCost(_) => runtime::handle_session_cost(&ctx).await,
        ClientRequest::SessionStatus(_) => runtime::handle_session_status(&ctx).await,

        // === Turn control ===
        ClientRequest::TurnStart(params) => turn::handle_turn_start(params, &ctx).await,
        ClientRequest::TurnInterrupt(_) => turn::handle_turn_interrupt(&ctx).await,

        // === Running task observability ===
        ClientRequest::TaskList(_) => runtime::handle_task_list(&ctx).await,
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
        ClientRequest::ResetSessionPermissionRules(_) => {
            runtime::handle_reset_session_permission_rules(&ctx).await
        }
        ClientRequest::StopTask(params) => runtime::handle_stop_task(params, &ctx).await,
        ClientRequest::RewindFiles(params) => rewind::handle_rewind_files(params, &ctx).await,
        ClientRequest::UpdateEnv(params) => runtime::handle_update_env(params, &ctx).await,
        ClientRequest::BackgroundAllTasks(_) => runtime::handle_background_all_tasks(&ctx).await,

        // `keepAlive` is the simplest handler — respond with empty ok so
        // clients using it as a heartbeat get immediate acknowledgement.
        ClientRequest::KeepAlive => HandlerResult::ok_empty(),

        ClientRequest::CancelRequest(params) => turn::handle_cancel_request(params, &ctx).await,
        ClientRequest::AgentInterruptCurrentWork(params) => {
            runtime::handle_agent_interrupt_current_work(params, &ctx).await
        }

        // === Config ===
        ClientRequest::ConfigRead(params) => config::handle_config_read(params, &ctx).await,
        ClientRequest::ConfigWrite(params) => config::handle_config_write(params, &ctx).await,

        // === TS P1 gap additions ===
        ClientRequest::McpStatus(_) => mcp::handle_mcp_status(&ctx).await,
        ClientRequest::ContextUsage(_) => runtime::handle_context_usage(&ctx).await,
        ClientRequest::McpSetServers(params) => mcp::handle_mcp_set_servers(params, &ctx).await,
        ClientRequest::McpReconnect(params) => mcp::handle_mcp_reconnect(params, &ctx).await,
        ClientRequest::McpToggle(params) => mcp::handle_mcp_toggle(params, &ctx).await,
        ClientRequest::PluginReload(_) => runtime::handle_plugin_reload(&ctx).await,
        ClientRequest::HookReload(_) => runtime::handle_hook_reload(&ctx).await,
        ClientRequest::ConfigApplyFlags(params) => {
            runtime::handle_config_apply_flags(params, &ctx).await
        }
    }
}
