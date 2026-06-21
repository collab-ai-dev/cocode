use coco_types::CommandArgumentKind;
use coco_types::CommandBase;
use coco_types::CommandContext;
use coco_types::CommandSafety;
use coco_types::CommandType;
use coco_types::LocalCommandData;
use coco_types::PromptCommandData;

use super::*;

fn test_base(name: &str, description: &str, aliases: Vec<String>) -> CommandBase {
    CommandBase {
        name: name.into(),
        description: description.into(),
        aliases,
        availability: vec![],
        is_hidden: false,
        argument_hint: None,
        argument_kind: CommandArgumentKind::None,
        when_to_use: None,
        user_invocable: true,
        is_sensitive: false,
        loaded_from: None,
        safety: CommandSafety::default(),
        supports_non_interactive: false,
    }
}

#[test]
fn test_command_registry() {
    let mut registry = CommandRegistry::new();
    registry.register(RegisteredCommand {
        base: test_base("help", "Show help", vec!["h".into(), "?".into()]),
        command_type: CommandType::Prompt(PromptCommandData {
            progress_message: "Loading help...".into(),
            content_length: 0,
            allowed_tools: None,
            model: None,
            context: CommandContext::Inline,
            agent: None,
            thinking_level: None,
            hooks: None,
        }),
        handler: None,
        is_enabled: None,
    });

    assert_eq!(registry.len(), 1);
    assert!(registry.get("help").is_some());
    // Alias lookup
    assert!(registry.get("h").is_some());
    assert!(registry.get("?").is_some());
}

#[test]
fn test_visible_commands() {
    let mut registry = CommandRegistry::new();
    registry.register(RegisteredCommand {
        base: test_base("visible", "Visible", vec![]),
        command_type: CommandType::Local(LocalCommandData {
            handler: "v".into(),
        }),
        handler: None,
        is_enabled: None,
    });
    let mut hidden_base = test_base("hidden", "Hidden", vec![]);
    hidden_base.is_hidden = true;
    hidden_base.user_invocable = false;
    registry.register(RegisteredCommand {
        base: hidden_base,
        command_type: CommandType::Local(LocalCommandData {
            handler: "h".into(),
        }),
        handler: None,
        is_enabled: None,
    });

    assert_eq!(registry.visible().len(), 1);
    assert_eq!(registry.len(), 2);
}

#[test]
fn test_register_builtins() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    // Drift notes:
    // - /commit, /pr, /review were promoted out of register_builtins
    //   (/commit lives in register_ts_parity_handlers; /pr removed —
    //   upstream uses /commit-push-pr; /review is now a Prompt-type command
    //   registered in implementations.rs).
    // - /bug, /ant-trace are deliberately not ported;
    //   see commands/CLAUDE.md "Deliberately Not Ported".
    // - /login + /logout are registered (multi-provider OAuth subscriptions);
    //   the interactive flow is handled in app/cli::tui_runner.
    assert_eq!(registry.len(), 21);
    assert!(registry.get("help").is_some());
    assert!(registry.get("clear").is_some());
    assert!(registry.get("compact").is_some());
    assert!(registry.get("login").is_some());
    assert!(registry.get("logout").is_some());
    assert!(registry.get("config").is_some());
    assert!(registry.get("status").is_some());
    assert!(registry.get("model").is_some());
    assert!(registry.get("diff").is_some());
    assert!(registry.get("mcp").is_some());
    assert!(registry.get("doctor").is_some());
}

#[test]
fn test_lookup_by_alias() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    // "h" and "?" are aliases for "help"
    let by_alias = registry.get("h");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "help");

    let by_alias = registry.get("?");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "help");

    // "bashes" is the only alias for "tasks"; "status" has none, and the
    // coco-invented "st"/"todo" aliases were removed.
    let by_alias = registry.get("bashes");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "tasks");
    assert!(registry.get("st").is_none());
    assert!(registry.get("todo").is_none());

    // "settings" is alias for "config"
    let by_alias = registry.get("settings");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "config");
}

#[tokio::test]
async fn test_execute_help() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    let output = registry.execute("help", "").await.unwrap();
    assert!(output.contains("Available commands"));
}

#[tokio::test]
async fn test_execute_by_alias() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    let output = registry.execute("h", "").await.unwrap();
    assert!(output.contains("Available commands"));
}

#[tokio::test]
async fn test_execute_config_with_args() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    let output = registry.execute("config", "theme dark").await.unwrap();
    assert!(output.contains("theme dark"));
}

#[tokio::test]
async fn test_execute_unknown_command() {
    let registry = CommandRegistry::new();
    let result = registry.execute("nonexistent", "").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown command"));
}

#[tokio::test]
async fn test_execute_command_without_handler() {
    let mut registry = CommandRegistry::new();
    registry.register(RegisteredCommand {
        base: test_base("no-handler", "Has no handler", vec![]),
        command_type: CommandType::Local(LocalCommandData {
            handler: "none".into(),
        }),
        handler: None,
        is_enabled: None,
    });

    let result = registry.execute("no-handler", "").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no handler"));
}

#[tokio::test]
async fn test_builtin_command_handler_name() {
    let cmd = BuiltinCommand::new("test", |_| "ok".to_string());
    assert_eq!(cmd.handler_name(), "test");
    let result = cmd.execute("").await.unwrap();
    assert_eq!(result, "ok");
}

#[test]
fn test_all_builtins_are_visible() {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);

    // All built-in commands should be visible (not hidden). Count tracks
    // register_builtins (21 incl. /login + /logout — see test_register_builtins).
    assert_eq!(registry.visible().len(), 21);
}
