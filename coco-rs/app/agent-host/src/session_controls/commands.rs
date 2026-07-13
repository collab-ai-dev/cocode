use crate::session_runtime::SessionHandle;

pub async fn build_loop_command_prompt(session: &SessionHandle, args: &str) -> String {
    let cwd = session.current_cwd().read().await.clone();
    let runtime_config = session.runtime_config();
    coco_skills::bundled::loop_skill::prompt_for_command(
        args,
        session.original_cwd(),
        &cwd,
        runtime_config.loop_config.default_prompt_enabled,
        runtime_config.loop_config.dynamic_enabled,
        runtime_config.loop_config.persistent_preamble_enabled,
        runtime_config
            .features
            .enabled(coco_types::Feature::AgentTriggersRemote),
    )
}

pub async fn command_resolves_to(
    session: &SessionHandle,
    name: &str,
    canonical_name: &str,
) -> bool {
    session
        .current_command_registry()
        .await
        .get(name)
        .is_some_and(|command| command.base.name == canonical_name)
}
