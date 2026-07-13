use std::path::PathBuf;

use crate::session_runtime::{SessionHandle, SessionStartRuntimeConfig};

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionStartInput {
    pub(crate) cwd: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) permission_mode: Option<coco_types::PermissionMode>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedStartSession {
    pub(crate) session_id: coco_types::SessionId,
    pub(crate) cwd: String,
    pub(crate) model: String,
    pub(crate) permission_mode: Option<coco_types::PermissionMode>,
    pub(crate) agent_progress_summaries_enabled: bool,
    pub(crate) plan_mode_custom_instructions: Option<String>,
}

#[derive(Debug)]
pub(crate) enum PrepareSessionStartError {
    MissingWorkspaceCwd,
}

impl PrepareSessionStartError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::MissingWorkspaceCwd => {
                "workspace cwd is unavailable before session/start; provide session/start.cwd or install startup cwd"
            }
        }
    }
}

pub(crate) fn prepare_session_start(
    input: SessionStartInput,
    workspace_cwd: Option<PathBuf>,
    default_model: &str,
    connection_profile: &coco_types::ConnectionProfile,
) -> Result<PreparedStartSession, PrepareSessionStartError> {
    let session_id = coco_types::SessionId::generate();
    let cwd = match input.cwd.clone() {
        Some(cwd) => cwd,
        None => workspace_cwd
            .ok_or(PrepareSessionStartError::MissingWorkspaceCwd)?
            .to_string_lossy()
            .into_owned(),
    };
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| default_model.to_string());
    let initialize = connection_profile.initialize();
    Ok(PreparedStartSession {
        session_id,
        cwd,
        model,
        permission_mode: input.permission_mode,
        agent_progress_summaries_enabled: initialize.agent_progress_summaries.unwrap_or(false),
        plan_mode_custom_instructions: initialize.plan_mode_instructions.clone(),
    })
}

pub(crate) async fn apply_prepared_session_start(
    prepared: &PreparedStartSession,
    runtime: &SessionHandle,
) {
    runtime
        .apply_session_start_config(SessionStartRuntimeConfig {
            model_id: Some(prepared.model.clone()),
            permission_mode: prepared.permission_mode,
            agent_progress_summaries_enabled: prepared.agent_progress_summaries_enabled,
            plan_mode_custom_instructions: Some(prepared.plan_mode_custom_instructions.clone()),
            requires_structured_output: false,
        })
        .await;
    runtime.reset_session_accounting();
}
