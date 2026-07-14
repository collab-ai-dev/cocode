use std::path::PathBuf;

use crate::session_runtime::{SessionHandle, SessionStartRuntimeConfig};

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionStartInput {
    pub(crate) session_id: Option<coco_types::SessionId>,
    pub(crate) cwd: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) permission_mode: Option<coco_types::PermissionMode>,
    pub(crate) max_turns: Option<i32>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) append_system_prompt: Option<String>,
    pub(crate) json_schema: Option<serde_json::Value>,
    pub(crate) plan_mode_instructions: Option<String>,
    pub(crate) initial_messages: Vec<coco_messages::Message>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedStartSession {
    pub(crate) session_id: coco_types::SessionId,
    pub(crate) cwd: String,
    pub(crate) model: String,
    pub(crate) permission_mode: Option<coco_types::PermissionMode>,
    pub(crate) max_turns: Option<i32>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) append_system_prompt: Option<String>,
    pub(crate) json_schema: Option<serde_json::Value>,
    pub(crate) agent_progress_summaries_enabled: bool,
    pub(crate) plan_mode_custom_instructions: Option<String>,
    pub(crate) initial_messages: Vec<coco_messages::Message>,
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
    let session_id = input
        .session_id
        .clone()
        .unwrap_or_else(coco_types::SessionId::generate);
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
        max_turns: input.max_turns,
        max_budget_usd: input.max_budget_usd,
        system_prompt: input.system_prompt,
        append_system_prompt: input.append_system_prompt,
        json_schema: input.json_schema,
        agent_progress_summaries_enabled: initialize.agent_progress_summaries.unwrap_or(false),
        plan_mode_custom_instructions: input.plan_mode_instructions,
        initial_messages: input.initial_messages,
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
            max_turns: prepared.max_turns,
            max_budget_usd: prepared.max_budget_usd,
            system_prompt: prepared.system_prompt.clone(),
            append_system_prompt: prepared.append_system_prompt.clone(),
            agent_progress_summaries_enabled: prepared.agent_progress_summaries_enabled,
            plan_mode_custom_instructions: Some(prepared.plan_mode_custom_instructions.clone()),
            requires_structured_output: false,
        })
        .await;
    runtime.reset_session_accounting();
}
