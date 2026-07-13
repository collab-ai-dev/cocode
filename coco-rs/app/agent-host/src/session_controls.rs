use crate::session_runtime::{SessionHandle, SessionTaskError};

mod agent;
mod commands;
mod env;
mod fast_mode;
mod flags;
mod model;
mod observability;
mod permission;
mod reload;
mod rewind;
mod skill_overrides;
mod task;

pub use agent::set_agent_color;
pub use commands::{build_loop_command_prompt, command_resolves_to};
pub use env::{EnvUpdateResult, update_env};
pub use fast_mode::{next_fast_mode_state, set_fast_mode};
pub use flags::{
    plan_mode_feature_enabled, should_respond_to_bash_commands, should_restore_v1_todos_on_resume,
};
pub use model::{
    RestrictedModelSelection, SetModelResult, SetModelRoleResult, SetThinkingResult,
    moa_one_shot_model_runtime_source, restricted_model_selection_for_args, set_model,
    set_model_role, set_thinking,
};
pub use observability::{context_usage, session_cost, session_status};
pub use permission::{
    DirectoryAccessPreparationError, PermissionModeStatus, PermissionMutationAction,
    PermissionRuleResetResult, PermissionsMutation, PreparedDirectoryAccess,
    apply_permission_update, directory_already_accessible_message, parse_permissions_mutation,
    permission_mode_status, permission_mutation_action, prepare_directory_access_update,
    reset_permission_rules, set_permission_mode,
};
pub use reload::{reload_hooks, reload_plugins};
pub use rewind::{
    FileHistoryDiffTarget, file_history_diff, rewind_diff_stats, rewind_diff_stats_between,
    rewind_files,
};
pub use skill_overrides::{SkillOverridesUpdate, write_skill_overrides};
pub use task::{
    TurnInterruptResult, background_all_tasks, interrupt_active_turn, interrupt_agent_current_work,
    list_tasks, stop_task, task_detail,
};

#[derive(Debug, thiserror::Error)]
pub enum SessionControlError {
    #[error("{operation} requires an active session runtime")]
    ActiveRuntimeRequired { operation: &'static str },
    #[error("no active session; call session/start first")]
    NoActiveSession,
    #[error("task runtime is not available for this session")]
    TaskRuntimeUnavailable,
    #[error("{operation}: {source}")]
    Task {
        operation: &'static str,
        source: SessionTaskError,
    },
    #[error(
        "failed to apply {role} -> {provider}/{model_id}: {source}",
        role = role.as_str()
    )]
    ModelRole {
        role: coco_types::ModelRole,
        provider: String,
        model_id: String,
        source: anyhow::Error,
    },
    #[error("{0}")]
    ContextUsage(String),
    #[error("{0}")]
    HookReload(String),
    #[error("{0}")]
    AgentInterrupt(String),
    #[error("control/rewindFiles: file history not enabled on this server")]
    FileHistoryNotEnabled,
    #[error("control/rewindFiles: no snapshot for user_message_id {0}")]
    FileRewindSnapshotMissing(String),
    #[error("control/rewindFiles {context}: {source}")]
    FileRewindOperation {
        context: &'static str,
        source: anyhow::Error,
    },
    #[error("file history is not enabled for this session")]
    FileDiffNotEnabled,
    #[error("no snapshot found for message id {0}")]
    FileDiffSnapshotMissing(String),
    #[error("unable to build {context}: {source}")]
    FileDiffOperation {
        context: &'static str,
        source: anyhow::Error,
    },
    #[error("no active turn")]
    NoActiveTurn,
}

fn require_runtime(
    runtime: Option<SessionHandle>,
    operation: &'static str,
) -> Result<SessionHandle, SessionControlError> {
    runtime.ok_or(SessionControlError::ActiveRuntimeRequired { operation })
}

#[cfg(test)]
#[path = "session_controls.test.rs"]
mod tests;
