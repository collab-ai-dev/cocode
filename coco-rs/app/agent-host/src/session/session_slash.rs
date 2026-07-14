use std::sync::Arc;

use crate::session_runtime::SessionHandle;

pub fn fork_skill_result_body(name: &str, result: Result<String, String>) -> String {
    match result {
        Ok(output) => format!("<local-command-stdout>\n{output}\n</local-command-stdout>"),
        Err(error) => {
            tracing::warn!(skill = %name, error = %error, "fork-mode skill failed");
            format!(
                "<local-command-stderr>\nSkill '/{name}' failed: {error}\n</local-command-stderr>"
            )
        }
    }
}

pub async fn invoke_fork_skill_and_append_result(
    session: &SessionHandle,
    event_tx: tokio::sync::mpsc::Sender<coco_types::CoreEvent>,
    name: &str,
    args: &str,
) -> Vec<Arc<coco_messages::Message>> {
    let body = fork_skill_result_body(name, session.invoke_skill_fork(name, args).await);
    crate::session_messages::append_fork_skill_result_to_history_and_emit(
        session,
        event_tx,
        &crate::session_messages::slash_command_metadata(name, args),
        &body,
    )
    .await
}

pub struct ExecutableSlashCommand {
    pub canonical_name: String,
    pub command_type: coco_types::CommandType,
    pub handler: Arc<dyn coco_commands::CommandHandler>,
}

impl ExecutableSlashCommand {
    pub fn record_skill_invocation(&self, outcome: coco_skills::telemetry::SkillOutcome) {
        if !matches!(self.command_type, coco_types::CommandType::Prompt(_)) {
            return;
        }
        coco_skills::telemetry::record_invocation_outcome_detached(
            coco_config::global_config::config_home(),
            self.canonical_name.clone(),
            outcome,
        );
    }
}

pub enum ResolvedSlashCommand {
    NotFound,
    Inactive,
    Loop { canonical_name: String },
    ForkSkill { canonical_name: String },
    PromptWithoutHandler,
    NoHandler,
    Executable(Box<ExecutableSlashCommand>),
}

pub async fn resolve_registered_command(
    session: &SessionHandle,
    name: &str,
) -> ResolvedSlashCommand {
    let registry_snapshot = session.current_command_registry().await;
    resolve_registered_command_from_registry(&registry_snapshot, name)
}

pub fn resolve_registered_command_from_registry(
    registry: &coco_commands::CommandRegistry,
    name: &str,
) -> ResolvedSlashCommand {
    let Some(command) = registry.get(name) else {
        return ResolvedSlashCommand::NotFound;
    };
    if !command.is_active() {
        return ResolvedSlashCommand::Inactive;
    }
    let canonical_name = command.base.name.clone();
    if canonical_name == "loop" {
        return ResolvedSlashCommand::Loop { canonical_name };
    }
    if let coco_types::CommandType::Prompt(data) = &command.command_type
        && data.context == coco_types::CommandContext::Fork
    {
        return ResolvedSlashCommand::ForkSkill { canonical_name };
    }
    let Some(handler) = command.handler.clone() else {
        if matches!(command.command_type, coco_types::CommandType::Prompt(_)) {
            return ResolvedSlashCommand::PromptWithoutHandler;
        }
        return ResolvedSlashCommand::NoHandler;
    };
    ResolvedSlashCommand::Executable(Box::new(ExecutableSlashCommand {
        canonical_name,
        command_type: command.command_type.clone(),
        handler,
    }))
}

#[cfg(test)]
#[path = "session_slash.test.rs"]
mod tests;
