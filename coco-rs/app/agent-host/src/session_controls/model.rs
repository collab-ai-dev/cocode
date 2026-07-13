use crate::session_runtime::{SessionHandle, SessionModelRoleChange, SessionModelRoleSelection};

use super::{SessionControlError, require_runtime};

pub struct SetModelResult {
    pub session_id: coco_types::SessionId,
    pub old_model: String,
    pub new_model: String,
}

pub struct RestrictedModelSelection {
    pub provider: String,
    pub model_id: String,
}

pub struct SetModelRoleResult {
    pub session_id: coco_types::SessionId,
    pub changed: coco_types::ModelRoleChangedParams,
    pub display_name: String,
}

pub struct SetThinkingResult {
    pub session_id: coco_types::SessionId,
    pub changed: Option<coco_types::ModelRoleChangedParams>,
}

pub async fn set_model(
    runtime: Option<SessionHandle>,
    new_model: String,
) -> Result<SetModelResult, SessionControlError> {
    let runtime = require_runtime(runtime, "control/setModel")?;
    let session_id = runtime.session_id().clone();
    let old_model = runtime.set_model_id(new_model.clone()).await;
    Ok(SetModelResult {
        session_id,
        old_model,
        new_model,
    })
}

pub fn restricted_model_selection_for_args(
    session: &SessionHandle,
    args: &str,
) -> Option<RestrictedModelSelection> {
    let available_models = session
        .runtime_config()
        .settings
        .merged
        .available_models
        .as_deref();
    let resolved = coco_commands::handlers::model::resolve_model(args)?;
    if coco_config::is_model_allowed(resolved.provider, &resolved.model_id, available_models) {
        return None;
    }
    Some(RestrictedModelSelection {
        provider: resolved.provider.to_string(),
        model_id: resolved.model_id,
    })
}

pub fn moa_one_shot_model_runtime_source(
    session: &SessionHandle,
) -> coco_inference::ModelRuntimeSource {
    coco_inference::ModelRuntimeSource::Explicit(coco_types::ProviderModelSelection {
        provider: coco_config::MOA_PROVIDER.to_string(),
        model_id: session
            .runtime_config()
            .settings
            .merged
            .moa
            .default_preset_name()
            .to_string(),
    })
}

pub async fn set_model_role(
    runtime: Option<SessionHandle>,
    role: coco_types::ModelRole,
    provider: String,
    model_id: String,
    effort: Option<coco_types::ReasoningEffort>,
) -> Result<SetModelRoleResult, SessionControlError> {
    let runtime = require_runtime(runtime, "control/setModelRole")?;
    let session_id = runtime.session_id().clone();
    let change = runtime
        .apply_model_role_selection(SessionModelRoleSelection {
            role,
            provider: provider.clone(),
            model_id: model_id.clone(),
            effort,
        })
        .await
        .map_err(|source| SessionControlError::ModelRole {
            role,
            provider,
            model_id,
            source,
        })?;
    let display_name = change.display_name.clone();
    Ok(SetModelRoleResult {
        session_id,
        changed: model_role_changed_params(change),
        display_name,
    })
}

pub async fn set_thinking(
    runtime: Option<SessionHandle>,
    active_session_id: Option<coco_types::SessionId>,
    thinking_level: Option<coco_types::ThinkingLevel>,
) -> Result<SetThinkingResult, SessionControlError> {
    let Some(runtime) = runtime else {
        let Some(session_id) = active_session_id else {
            return Err(SessionControlError::NoActiveSession);
        };
        return Ok(SetThinkingResult {
            session_id,
            changed: None,
        });
    };
    let session_id = runtime.session_id().clone();
    runtime.set_thinking_level(thinking_level.clone()).await;
    let changed = runtime
        .model_role_change_snapshot(
            coco_types::ModelRole::Main,
            thinking_level.map(|level| level.effort),
        )
        .await
        .map(model_role_changed_params);
    Ok(SetThinkingResult {
        session_id,
        changed,
    })
}

fn model_role_changed_params(change: SessionModelRoleChange) -> coco_types::ModelRoleChangedParams {
    coco_types::ModelRoleChangedParams {
        role: change.role,
        model_id: change.display_model_id,
        provider: change.display_provider,
        context_window: change.context_window,
        effort: change.effort,
    }
}
