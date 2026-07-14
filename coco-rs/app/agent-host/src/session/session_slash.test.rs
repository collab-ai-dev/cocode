use std::sync::Arc;

use async_trait::async_trait;
use coco_commands::CommandRegistry;
use coco_commands::RegisteredCommand;

use super::*;

struct TestHandler;

#[async_trait]
impl coco_commands::CommandHandler for TestHandler {
    async fn execute(&self, _args: &str) -> coco_commands::Result<String> {
        Ok("ok".to_string())
    }

    fn handler_name(&self) -> &str {
        "test"
    }
}

fn disabled() -> bool {
    false
}

fn local_command(name: &str, handler: bool) -> RegisteredCommand {
    RegisteredCommand {
        base: coco_commands::builtin_base(name, "test command", &[]),
        command_type: coco_types::CommandType::Local(coco_types::LocalCommandData {
            handler: name.to_string(),
        }),
        handler: handler.then(|| Arc::new(TestHandler) as Arc<dyn coco_commands::CommandHandler>),
        is_enabled: None,
    }
}

fn prompt_command(
    name: &str,
    context: coco_types::CommandContext,
    handler: bool,
) -> RegisteredCommand {
    RegisteredCommand {
        base: coco_commands::builtin_base(name, "test prompt", &[]),
        command_type: coco_types::CommandType::Prompt(coco_types::PromptCommandData {
            progress_message: String::new(),
            content_length: 0,
            allowed_tools: None,
            model: None,
            context,
            agent: None,
            thinking_level: None,
            hooks: None,
        }),
        handler: handler.then(|| Arc::new(TestHandler) as Arc<dyn coco_commands::CommandHandler>),
        is_enabled: None,
    }
}

#[test]
fn resolves_not_found() {
    let registry = CommandRegistry::new();
    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "missing"),
        ResolvedSlashCommand::NotFound
    ));
}

#[test]
fn resolves_inactive_before_special_cases() {
    let mut registry = CommandRegistry::new();
    let mut command = local_command("loop", true);
    command.is_enabled = Some(disabled);
    registry.register(command);

    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "loop"),
        ResolvedSlashCommand::Inactive
    ));
}

#[test]
fn resolves_loop_by_canonical_name() {
    let mut registry = CommandRegistry::new();
    let mut command = local_command("loop", true);
    command.base.aliases.push("repeat".to_string());
    registry.register(command);

    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "repeat"),
        ResolvedSlashCommand::Loop { canonical_name } if canonical_name == "loop"
    ));
}

#[test]
fn resolves_fork_prompt_before_handler_execution() {
    let mut registry = CommandRegistry::new();
    registry.register(prompt_command(
        "forked",
        coco_types::CommandContext::Fork,
        true,
    ));

    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "forked"),
        ResolvedSlashCommand::ForkSkill { canonical_name } if canonical_name == "forked"
    ));
}

#[test]
fn resolves_prompt_without_handler_as_fallthrough() {
    let mut registry = CommandRegistry::new();
    registry.register(prompt_command(
        "prompt",
        coco_types::CommandContext::Inline,
        false,
    ));

    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "prompt"),
        ResolvedSlashCommand::PromptWithoutHandler
    ));
}

#[test]
fn resolves_local_without_handler_as_no_handler() {
    let mut registry = CommandRegistry::new();
    registry.register(local_command("local", false));

    assert!(matches!(
        resolve_registered_command_from_registry(&registry, "local"),
        ResolvedSlashCommand::NoHandler
    ));
}

#[test]
fn resolves_executable_with_canonical_name_and_command_type() {
    let mut registry = CommandRegistry::new();
    let mut command = local_command("canonical", true);
    command.base.aliases.push("alias".to_string());
    registry.register(command);

    let ResolvedSlashCommand::Executable(command) =
        resolve_registered_command_from_registry(&registry, "alias")
    else {
        panic!("expected executable command");
    };
    assert_eq!(command.canonical_name, "canonical");
    assert!(matches!(
        command.command_type,
        coco_types::CommandType::Local(_)
    ));
}

#[test]
fn fork_skill_result_body_wraps_success_output() {
    let body = fork_skill_result_body("demo", Ok("done".to_string()));

    assert_eq!(
        body,
        "<local-command-stdout>\ndone\n</local-command-stdout>"
    );
}

#[test]
fn fork_skill_result_body_wraps_failure_output() {
    let body = fork_skill_result_body("demo", Err("boom".to_string()));

    assert_eq!(
        body,
        "<local-command-stderr>\nSkill '/demo' failed: boom\n</local-command-stderr>"
    );
}
