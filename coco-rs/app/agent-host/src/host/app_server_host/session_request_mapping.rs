use super::session_operation_input::{SessionReplaceDestination, SessionReplaceInput};

pub(crate) fn session_start_input_from_params(
    params: &coco_types::SessionStartParams,
) -> crate::session_start::SessionStartInput {
    crate::session_start::SessionStartInput {
        session_id: params.session_id.clone(),
        cwd: params.cwd.clone(),
        model: params.model.clone(),
        permission_mode: params.permission_mode,
        max_turns: params.max_turns,
        max_budget_usd: params.max_budget_usd,
        system_prompt: params.system_prompt.clone(),
        append_system_prompt: params.append_system_prompt.clone(),
        json_schema: params.json_schema.clone(),
        plan_mode_instructions: params.plan_mode_instructions.clone(),
        initial_messages: params.initial_messages.clone(),
    }
}

pub(crate) fn session_resume_input_from_params(
    params: &coco_types::SessionResumeParams,
) -> crate::session_resume::SessionResumeInput {
    crate::session_resume::SessionResumeInput {
        target: params.target.clone(),
        plan_mode_instructions: params.plan_mode_instructions.clone(),
    }
}

pub(crate) fn session_replace_input_from_params(
    params: &coco_types::SessionReplaceParams,
) -> SessionReplaceInput {
    SessionReplaceInput {
        source: params.source.clone(),
        destination: match &params.destination {
            coco_types::SessionReplacement::Fresh(params) => {
                SessionReplaceDestination::Fresh(Box::new(session_start_input_from_params(params)))
            }
            coco_types::SessionReplacement::Resume(target) => {
                SessionReplaceDestination::Resume(target.clone())
            }
            coco_types::SessionReplacement::Clear => SessionReplaceDestination::Clear,
        },
    }
}
