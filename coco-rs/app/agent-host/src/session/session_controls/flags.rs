use crate::session_runtime::SessionHandle;

pub fn plan_mode_feature_enabled(session: &SessionHandle) -> bool {
    session
        .runtime_config()
        .features
        .enabled(coco_types::Feature::PlanMode)
}

pub fn should_restore_v1_todos_on_resume(session: &SessionHandle) -> bool {
    !session
        .runtime_config()
        .features
        .enabled(coco_types::Feature::TaskV2)
}

pub fn should_respond_to_bash_commands(session: &SessionHandle) -> bool {
    session
        .runtime_config()
        .settings
        .merged
        .respond_to_bash_commands
        .unwrap_or(true)
}
