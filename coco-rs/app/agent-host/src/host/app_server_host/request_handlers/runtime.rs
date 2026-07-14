//! Runtime-state mutations (`setModel` / `setModelRole` / `setPermissionMode`
//! / `setThinking` / `applyPermissionUpdate` / `updateEnv` / `stopTask`) plus
//! observability and runtime-backed handlers (`context/usage`,
//! `plugin/reload`, `hook/reload`, `config/applyFlags`).

mod env;
mod model;
mod observability;
mod permission;
mod reload;
mod task;

pub(crate) use env::handle_update_env;
pub(crate) use model::{
    handle_set_agent_color, handle_set_model, handle_set_model_role, handle_set_thinking,
};
pub(crate) use observability::{handle_context_usage, handle_session_cost, handle_session_status};
pub(crate) use permission::{
    handle_apply_permission_update, handle_reset_session_permission_rules,
    handle_set_permission_mode,
};
pub(crate) use reload::{handle_config_apply_flags, handle_hook_reload, handle_plugin_reload};
pub(crate) use task::{
    handle_agent_interrupt_current_work, handle_background_all_tasks, handle_stop_task,
    handle_task_detail, handle_task_list,
};

use super::HandlerResult;
use crate::session_controls::SessionControlError;

fn session_control_error(error: SessionControlError) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.to_string(),
        data: None,
    }
}
