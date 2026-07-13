use super::session_operation_input::{SessionReplaceDestination, SessionReplaceInput};

pub(crate) fn session_start_input_from_params(
    params: &coco_types::SessionStartParams,
) -> crate::session_start::SessionStartInput {
    crate::session_start::SessionStartInput {
        cwd: params.cwd.clone(),
        model: params.model.clone(),
        permission_mode: params.permission_mode,
    }
}

pub(crate) fn session_resume_input_from_params(
    params: &coco_types::SessionResumeParams,
) -> crate::session_resume::SessionResumeInput {
    crate::session_resume::SessionResumeInput {
        target: params.target.clone(),
    }
}

pub(crate) fn session_replace_input_from_params(
    params: &coco_types::SessionReplaceParams,
) -> SessionReplaceInput {
    SessionReplaceInput {
        source: params.source.clone(),
        destination: match &params.destination {
            coco_types::SessionReplacement::Fresh(params) => {
                SessionReplaceDestination::Fresh(session_start_input_from_params(params))
            }
            coco_types::SessionReplacement::Resume(target) => {
                SessionReplaceDestination::Resume(target.clone())
            }
        },
    }
}
