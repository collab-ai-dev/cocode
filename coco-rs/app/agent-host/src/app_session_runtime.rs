use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use coco_app_runtime::ProcessRuntime;
use coco_messages::Message;
use coco_types::{AgentDefinition, SessionCallbackRequirements, SessionId};

use crate::session_runtime::{SessionHandle, SessionRuntimeFactory, SessionStartRuntimeConfig};

#[derive(Clone)]
pub(crate) struct AppSessionRuntimeBinding {
    pub(crate) runtime_factory: SessionRuntimeFactory,
    pub(crate) process_runtime: Arc<ProcessRuntime>,
    pub(crate) cwd: PathBuf,
    pub(crate) integration_options: crate::session_bootstrap::SessionIntegrationOptions,
}

#[derive(Clone)]
pub(crate) struct AppSessionRuntimeProfile {
    pub(crate) callback_requirements: SessionCallbackRequirements,
    pub(crate) plan_mode_custom_instructions: Option<String>,
    pub(crate) supplied_agents: Vec<AgentDefinition>,
    pub(crate) requires_structured_output: bool,
}

pub(crate) fn session_build_cwd_from_str(base: &std::path::Path, cwd: &str) -> PathBuf {
    session_build_cwd(base, std::path::Path::new(cwd))
}

pub(crate) fn session_build_cwd(base: &std::path::Path, cwd: &std::path::Path) -> PathBuf {
    if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        base.join(cwd)
    }
}

pub(crate) async fn build_app_session_runtime_for_start(
    binding: &AppSessionRuntimeBinding,
    profile: &AppSessionRuntimeProfile,
    prepared: &crate::session_start::PreparedStartSession,
) -> Result<SessionHandle> {
    let session = binding
        .runtime_factory
        .build_with_session_id_and_cwd(
            prepared.session_id.clone(),
            session_build_cwd_from_str(&binding.cwd, &prepared.cwd),
        )
        .await?;
    apply_app_session_runtime_profile(profile, &session).await;
    Ok(session)
}

pub(crate) async fn build_app_session_runtime_for_resume(
    binding: &AppSessionRuntimeBinding,
    profile: &AppSessionRuntimeProfile,
    session_id: SessionId,
    cwd: PathBuf,
) -> Result<SessionHandle> {
    let session = binding
        .runtime_factory
        .build_with_session_id_and_cwd(session_id, session_build_cwd(&binding.cwd, &cwd))
        .await?;
    apply_app_session_runtime_profile(profile, &session).await;
    Ok(session)
}

pub(crate) async fn apply_app_session_runtime_profile(
    profile: &AppSessionRuntimeProfile,
    session: &SessionHandle,
) {
    session.install_callback_requirements(profile.callback_requirements.clone());
    session
        .apply_session_start_config(SessionStartRuntimeConfig {
            model_id: None,
            permission_mode: None,
            agent_progress_summaries_enabled: false,
            plan_mode_custom_instructions: Some(profile.plan_mode_custom_instructions.clone()),
            requires_structured_output: profile.requires_structured_output,
        })
        .await;
    if !profile.supplied_agents.is_empty() {
        session
            .set_client_supplied_agents(profile.supplied_agents.clone())
            .await;
    }
}

pub(crate) async fn install_app_session_integrations(
    binding: &AppSessionRuntimeBinding,
    session: SessionHandle,
) -> Result<()> {
    let session_cwd = session.original_cwd().clone();
    crate::session_bootstrap::install_session_integrations(
        session,
        &session_cwd,
        Arc::clone(&binding.process_runtime),
        binding.integration_options.clone(),
    )
    .await
}

pub(crate) async fn hydrate_app_session_history(
    session: &SessionHandle,
    session_id: &SessionId,
    messages: &[Message],
) {
    crate::runtime_resume::hydrate_runtime_for_resume(session, session_id, messages).await;
}
