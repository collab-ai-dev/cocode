use super::*;
use crate::CommandRegistry;
use crate::handlers;
use crate::register_builtins;

#[test]
fn test_register_extended_builtins() {
    let mut registry = CommandRegistry::new();
    register_extended_builtins(&mut registry);

    // Verify we registered a reasonable number of extended commands.
    // Count drifts as commands move between layers; the floor only
    // catches whole-block regressions. Floor lowered to 42 after `/effort`
    // moved to the live Ctrl+T thinking-effort cycle.
    assert!(
        registry.len() >= 42,
        "Expected at least 42 extended commands, got {}",
        registry.len()
    );
}

#[test]
fn btw_registration_matches_upstream_command_metadata() {
    let mut registry = CommandRegistry::new();
    register_extended_builtins(&mut registry);

    let btw = registry.get(names::BTW).expect("btw registered");
    assert_eq!(
        btw.base.description,
        "Ask a quick side question without interrupting the main conversation"
    );
    assert_eq!(btw.base.argument_hint.as_deref(), Some("<question>"));
    assert_eq!(
        btw.base.argument_kind,
        coco_types::CommandArgumentKind::FreeText
    );
    assert_eq!(btw.base.safety, coco_types::CommandSafety::AlwaysSafe);
}

#[test]
fn test_extended_builtins_no_overlap_with_base() {
    let mut base_registry = CommandRegistry::new();
    register_builtins(&mut base_registry);

    let mut ext_registry = CommandRegistry::new();
    register_extended_builtins(&mut ext_registry);
    // `/review`, `/security-review`, `/insights` etc. moved to the
    // P1 handler set (Prompt-type). Include them so `key_commands`
    // below stays meaningful when checking the "post-extension" surface.
    register_ts_parity_handlers(
        &mut ext_registry,
        coco_types::UserType::Human,
        coco_types::Features::empty(),
        std::path::PathBuf::from("."),
        std::path::PathBuf::from("."),
        None,
    );

    // Count entries in both registries
    let base_count = base_registry.all().count();
    let ext_count = ext_registry.all().count();

    // Some commands exist in both (the extended set overrides/replaces them)
    // That's by design: extended handlers have real logic replacing stubs.
    // Verify the extended set has the key commands.
    let key_commands = [
        "compact",
        "context",
        "cost",
        "diff",
        "permissions",
        "session",
        "resume",
        "init",
        "doctor",
        "mcp",
        "plugin",
        "review",
    ];

    for cmd in &key_commands {
        assert!(
            ext_registry.get(cmd).is_some(),
            "extended registry missing key command: {cmd}"
        );
    }

    // Verify base set has its own commands
    assert!(base_registry.get("help").is_some());
    assert!(base_registry.get("clear").is_some());
    let clear = ext_registry.get("clear").expect("extended clear exists");
    assert!(
        clear.base.aliases.iter().any(|a| a == "reset")
            && clear.base.aliases.iter().any(|a| a == "new"),
        "/clear should expose reset/new aliases"
    );
    // Aliases resolve back to the canonical /clear via registry lookup.
    assert_eq!(
        ext_registry.get("reset").map(|c| c.base.name.as_str()),
        Some("clear"),
        "/reset should resolve to canonical /clear"
    );
    assert_eq!(
        ext_registry.get("new").map(|c| c.base.name.as_str()),
        Some("clear"),
        "/new should resolve to canonical /clear"
    );

    // Quick sanity: base and extended together should not panic
    let _base_count = base_count;
    let _ext_count = ext_count;
}

#[test]
fn test_all_name_constants_are_valid() {
    // Verify every name constant is non-empty and uses kebab-case ASCII.
    let all_names: &[&str] = &[
        names::HELP,
        names::CLEAR,
        names::COMPACT,
        names::STATUS,
        names::GOAL,
        names::EXIT,
        names::VERSION,
        names::CONFIG,
        names::MODEL,
        names::PERMISSIONS,
        names::THEME,
        names::COLOR,
        names::VIM,
        names::OUTPUT_STYLE,
        names::KEYBINDINGS,
        names::SANDBOX,
        names::SESSION,
        names::RESUME,
        names::COST,
        names::CONTEXT,
        names::RENAME,
        names::BRANCH,
        names::EXPORT,
        names::COPY,
        names::REWIND,
        names::STATS,
        names::DIFF,
        names::COMMIT,
        names::REVIEW,
        names::INIT,
        names::MCP,
        names::PLUGIN,
        names::AGENTS,
        names::TASKS,
        names::WORKFLOW,
        names::SKILLS,
        names::HOOKS,
        names::FILES,
        names::DOCTOR,
        names::UPGRADE,
        names::USAGE,
        names::BTW,
        names::MEMORY,
        names::PLAN,
        names::ADD_DIR,
        names::IDE,
        names::TAG,
        names::SUMMARY,
        names::PR_COMMENTS,
        names::PASSES,
        names::STATUSLINE,
        names::RELOAD_PLUGINS,
        names::SECURITY_REVIEW,
        names::INSIGHTS,
        names::ENV,
        names::DEBUG_TOOL_CALL,
    ];

    for name in all_names {
        assert!(!name.is_empty(), "found empty command name constant");
        assert!(
            name.chars()
                .all(|c: char| c.is_ascii_lowercase() || c == '-'),
            "command name '{name}' contains invalid characters"
        );
    }

    // After parity-trimming we keep ~50 constants; the floor catches future
    // accidental drops without hard-coding the precise count.
    assert!(
        all_names.len() >= 50,
        "expected at least 50 command name constants, got {}",
        all_names.len()
    );
}

#[test]
fn goal_handler_emits_typed_sentinels() {
    assert_eq!(
        parse_goal_sentinel(&goal_handler("")),
        Some(GoalCommandRequest::Status)
    );
    assert_eq!(
        parse_goal_sentinel(&goal_handler("clear")),
        Some(GoalCommandRequest::Clear)
    );
    for keyword in ["stop", "off", "reset", "none", "cancel"] {
        assert_eq!(
            parse_goal_sentinel(&goal_handler(keyword)),
            Some(GoalCommandRequest::Clear),
            "{keyword} should clear the active goal"
        );
    }
    assert_eq!(
        parse_goal_sentinel(&goal_handler("finish the migration")),
        Some(GoalCommandRequest::Set {
            condition: "finish the migration".to_string()
        })
    );
}

#[test]
fn goal_registration_matches_upstream_interactive_metadata() {
    let mut registry = CommandRegistry::new();
    register_extended_builtins(&mut registry);

    let goal = registry.get(names::GOAL).expect("/goal registered");
    assert_eq!(
        goal.base.description,
        "Set a goal Claude checks before stopping"
    );
    assert_eq!(
        goal.base.argument_hint.as_deref(),
        Some("[<condition> | clear]")
    );
}

#[test]
fn goal_handler_rejects_conditions_over_upstream_limit() {
    let output = goal_handler(&"x".repeat(4001));
    assert_eq!(
        output,
        "Goal condition is limited to 4000 characters (got 4001)"
    );
    assert!(parse_goal_sentinel(&output).is_none());
}

#[tokio::test]
async fn workflow_prompt_command_uses_workflow_tool() {
    let mut registry = CommandRegistry::new();
    register_ts_parity_handlers(
        &mut registry,
        coco_types::UserType::Human,
        coco_types::Features::with_defaults(),
        std::path::PathBuf::from("."),
        std::path::PathBuf::from("."),
        None,
    );

    let command = registry
        .get(names::WORKFLOW)
        .expect("/workflow should be registered");
    assert_eq!(
        registry.get("workflows").map(|c| c.base.name.as_str()),
        Some(names::WORKFLOW),
        "/workflows should resolve to canonical /workflow"
    );
    let CommandType::Prompt(data) = &command.command_type else {
        panic!("expected prompt command");
    };
    let allowed_tools = data.allowed_tools.as_ref().expect("allowed tools");
    assert_eq!(allowed_tools.len(), 1);
    assert_eq!(allowed_tools[0], "Workflow");

    let result = registry
        .execute_command(names::WORKFLOW, "name=analyze args={\"scope\":\"src\"}")
        .await
        .unwrap();
    let crate::CommandResult::Prompt { parts, .. } = result else {
        panic!("expected prompt result");
    };
    let text = match &parts[0] {
        crate::PromptPart::Text { text } => text,
        crate::PromptPart::File { .. } => panic!("expected text prompt part"),
    };
    assert!(text.contains("Workflow tool"));
    assert!(text.contains("## Task"));
    assert!(text.contains("name=analyze"));

    let result = registry.execute_command(names::WORKFLOW, "").await.unwrap();
    assert!(
        matches!(
            result,
            crate::CommandResult::OpenDialog(crate::DialogSpec::WorkflowPicker)
        ),
        "bare /workflow should open the workflow picker"
    );
}

#[tokio::test]
async fn review_prompt_command_matches_pr_scope_and_medium_effort() {
    let mut registry = CommandRegistry::new();
    register_ts_parity_handlers(
        &mut registry,
        coco_types::UserType::Human,
        coco_types::Features::with_defaults(),
        std::path::PathBuf::from("."),
        std::path::PathBuf::from("."),
        None,
    );

    let command = registry
        .get(names::REVIEW)
        .expect("/review should be registered");
    assert_eq!(
        command.base.description,
        "Review a GitHub pull request; for your working diff use /code-review"
    );
    assert_eq!(command.base.argument_hint.as_deref(), Some("[pr number]"));
    let CommandType::Prompt(data) = &command.command_type else {
        panic!("expected prompt command");
    };
    assert_eq!(
        data.thinking_level.as_ref().map(|level| level.effort),
        Some(coco_types::ReasoningEffort::Medium)
    );

    let result = registry
        .execute_command(names::REVIEW, "#123 focus security")
        .await
        .unwrap();
    let crate::CommandResult::Prompt { parts, .. } = result else {
        panic!("expected prompt result");
    };
    let text = match &parts[0] {
        crate::PromptPart::Text { text } => text,
        crate::PromptPart::File { .. } => panic!("expected text prompt part"),
    };
    assert!(text.contains("Review target: GitHub pull request `123`."));
    assert!(text.contains(
        "gh pr view 123 --json title,body,author,baseRefName,headRefName,state,additions,deletions,changedFiles,labels"
    ));
    assert!(text.contains("gh pr diff 123"));
    assert!(text.contains("Local working-tree changes are out of scope."));
    assert!(text.contains("Additional instructions from the user: focus security"));

    let no_arg = registry.execute_command(names::REVIEW, "").await.unwrap();
    let crate::CommandResult::Prompt { parts, .. } = no_arg else {
        panic!("expected prompt result");
    };
    let no_arg_text = match &parts[0] {
        crate::PromptPart::Text { text } => text,
        crate::PromptPart::File { .. } => panic!("expected text prompt part"),
    };
    assert!(no_arg_text.contains("gh pr list"));
    assert!(no_arg_text.contains("/review <number>"));
}

#[test]
fn test_plan_handler_subcommands() {
    // Fallback handler used when not running through the TUI dispatcher.
    assert!(plan_handler("").contains("Plan mode"));
    assert!(plan_handler("open").contains("EDITOR"));
    assert!(plan_handler("refactor the auth module").contains("Creating plan"));
    assert!(plan_handler("refactor the auth module").contains("EnterPlanMode"));
}

#[test]
fn statusline_prompt_describes_coco_runtime_fields() {
    let prompt = statusline_prompt();
    assert!(prompt.contains(coco_config::constants::PRODUCT_NAME));
    assert!(prompt.contains(&format!(
        "~/{}/settings.json",
        coco_utils_common::COCO_CONFIG_DIR_NAME
    )));
    assert!(prompt.contains("model.{id,display_name,provider}"));
    assert!(prompt.contains("context_window.{used,total,percent}"));
    assert!(prompt.contains("mcp.connected_servers"));
    assert!(!prompt.contains("~/.claude"));
    assert!(!prompt.contains("session_name"));
    assert!(!prompt.contains("transcript_path"));
    assert!(!prompt.contains("rate_limits"));
    assert!(!prompt.contains("worktree"));
}

#[tokio::test]
async fn test_memory_handler() {
    let output = super::handlers::memory::handler("".to_string())
        .await
        .unwrap();
    assert!(output.contains("Memory Files"));
}

// `/rewind` no longer has an inline string handler — the duplicate
// registration in `register_extended_builtins` was removed and
// `register_ts_parity_handlers` now owns the only handler
// (`handlers::rewind::RewindHandler`). The TUI command palette
// intercepts before that handler is reached.

#[tokio::test]
async fn test_skills_handler() {
    // Real enumeration via SkillManager — bundled skills are always
    // present so the count line and the [bundled] source tag prove the
    // handler exercised the real loader rather than a static stub.
    // (`list` exercises the text path; the no-arg path opens the TUI
    // dialog which is covered by skills.test.rs.)
    use crate::CommandHandler;
    let output = crate::handlers::skills::SkillsHandler
        .execute_command("list")
        .await
        .unwrap();
    match output {
        crate::CommandResult::Text(out) => {
            assert!(out.contains("skill(s) loaded"));
            assert!(out.contains("[bundled]"));
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[tokio::test]
async fn test_hooks_handler() {
    let output = super::handlers::hooks::handler("".to_string())
        .await
        .unwrap();
    assert!(!output.is_empty());
}

#[test]
fn test_sandbox_handler() {
    // Empty args lists modes + invocation hint.
    let listing = sandbox_handler("");
    assert!(listing.contains("Sandbox mode"));
    assert!(listing.contains("none"));
    assert!(listing.contains("readonly"));
    assert!(listing.contains("strict"));
    // Unknown subcommand surfaces a usage error without writing settings.
    assert!(sandbox_handler("bogus").contains("Unknown sandbox mode"));
}

#[test]
fn test_version_handler() {
    let output = version_handler("");
    let expected_prefix = format!("{} v", coco_config::constants::PRODUCT_NAME);
    assert!(output.starts_with(&expected_prefix));
}

// /vim is now an async handler; behavior covered by handlers::vim::tests.

// `/theme` moved to a CommandHandler (handlers::theme::ThemeHandler) that
// opens the picker on no args; see handlers/theme.test.rs.

#[test]
fn test_config_read_handler_accepts_jsonc_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
  // user comment
  "language": "zh-CN",
}
"#,
    )
    .unwrap();

    let output = config_read_handler_at_path(&path, "language");
    assert_eq!(output, r#"Current value of `language`: "zh-CN""#);
}

/// `/output-style` is the deprecated stub. The handler must:
///   1. Return the deprecation message regardless of args.
///   2. Be registered in the extended-builtins registry.
///   3. Be marked `is_hidden: true` so it doesn't show up in
///      typeahead/picker.
///   4. Stay reachable by name via `/help output-style` lookups.
#[test]
fn test_output_style_handler_returns_deprecation_message() {
    // Args ignored — handler accepts but ignores them.
    let expected = "/output-style has been deprecated. Use /config to change your output style, \
                    or set it in your settings file. Changes take effect on the next session.";
    assert_eq!(output_style_handler(""), expected);
    assert_eq!(output_style_handler("Explanatory"), expected);
    assert_eq!(output_style_handler("anything else"), expected);
}

#[test]
fn test_output_style_command_registered_and_hidden() {
    let mut registry = CommandRegistry::new();
    register_extended_builtins(&mut registry);

    let cmd = registry
        .get(names::OUTPUT_STYLE)
        .expect("/output-style must be registered");
    assert_eq!(cmd.base.name, "output-style");
    assert!(cmd.base.is_hidden, "/output-style must be hidden");
    assert_eq!(
        cmd.base.description,
        "Deprecated: use /config to change output style"
    );
    assert!(
        cmd.handler.is_some(),
        "/output-style must carry the deprecation handler"
    );
    // Visibility filters drop it from typeahead — verify behavior, not
    // just the flag.
    assert!(
        !registry
            .visible()
            .iter()
            .any(|c| c.base.name == names::OUTPUT_STYLE),
        "/output-style must not appear in registry.visible()"
    );
}

#[test]
fn test_color_handler_empty_lists_palette() {
    // Empty args: list the available color palette.
    let out = color_handler("");
    assert!(out.starts_with("Please provide a color"));
    for c in [
        "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan", "default",
    ] {
        assert!(out.contains(c), "missing color '{c}' in: {out}");
    }
}

#[test]
fn test_color_handler_valid_colors_case_insensitive() {
    // Both lowercase and uppercase resolve to the canonical lowercase name.
    assert_eq!(color_handler("red"), "Session color set to: red");
    assert_eq!(color_handler("RED"), "Session color set to: red");
    assert_eq!(color_handler("Cyan"), "Session color set to: cyan");
}

#[test]
fn test_color_handler_reset_aliases() {
    // Reset aliases: 'default','reset','none','gray','grey'.
    for alias in ["default", "reset", "none", "gray", "grey", "DEFAULT"] {
        assert_eq!(
            color_handler(alias),
            "Session color reset to default",
            "alias {alias} should reset"
        );
    }
}

#[test]
fn test_color_handler_invalid_color() {
    let out = color_handler("magenta");
    assert!(out.starts_with("Invalid color \"magenta\""), "{out}");
    assert!(out.contains("default"));
}

// /fast removed per parity scope; coverage dropped accordingly.

#[tokio::test]
async fn test_model_handler_empty_opens_picker() {
    use crate::CommandHandler;
    use crate::CommandResult;
    use crate::DialogSpec;
    let handler = crate::handlers::model::ModelHandler;
    let result = handler.execute_command("").await.unwrap();
    assert!(matches!(
        result,
        CommandResult::OpenDialog(DialogSpec::ModelPicker)
    ));
}

#[tokio::test]
async fn test_model_handler_known() {
    use crate::CommandHandler;
    use crate::CommandResult;
    // Sandbox the settings write so this test doesn't pollute the
    // developer's real `config home/settings.json`.
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("COCO_CONFIG_DIR");
    unsafe {
        std::env::set_var("COCO_CONFIG_DIR", tmp.path());
    }
    let handler = crate::handlers::model::ModelHandler;
    let result = handler.execute_command("sonnet").await.unwrap();
    unsafe {
        match prev {
            Some(v) => std::env::set_var("COCO_CONFIG_DIR", v),
            None => std::env::remove_var("COCO_CONFIG_DIR"),
        }
    }
    let text = match result {
        CommandResult::Text(t) => t,
        other => panic!("expected Text, got {other:?}"),
    };
    assert!(text.contains("Set Main"), "missing 'Set Main' in {text}");
    assert!(text.contains("anthropic/claude-sonnet-4-6"));
    assert!(text.contains("persisted to"));
}

#[tokio::test]
async fn test_model_handler_unknown() {
    use crate::CommandHandler;
    use crate::CommandResult;
    let handler = crate::handlers::model::ModelHandler;
    let result = handler.execute_command("gpt-4").await.unwrap();
    let text = match result {
        CommandResult::Text(t) => t,
        other => panic!("expected Text, got {other:?}"),
    };
    assert!(text.contains("Unknown model"));
}

#[tokio::test]
async fn test_permissions_handler_empty() {
    let output = handlers::permissions::handler(String::new()).await.unwrap();
    assert!(output.contains("Permission Rules"));
    assert!(output.contains("allow"));
    assert!(output.contains("deny"));
}

#[tokio::test]
async fn test_permissions_handler_allow_non_tui_hint() {
    // Non-TUI handler returns hint pointing at TUI / settings.json.
    // The TUI dispatcher (`tui_runner::dispatch_permissions_mutation`)
    // intercepts this and mutates engine_config — verified separately.
    let output = handlers::permissions::handler("allow Bash".to_string())
        .await
        .unwrap();
    assert!(output.contains("Bash"));
    assert!(output.contains("only effective inside the TUI"));
}

#[tokio::test]
async fn test_permissions_handler_deny_non_tui_hint() {
    let output = handlers::permissions::handler("deny Write".to_string())
        .await
        .unwrap();
    assert!(output.contains("Write"));
    assert!(output.contains("only effective inside the TUI"));
}

#[tokio::test]
async fn test_permissions_handler_reset_non_tui_honest() {
    let output = handlers::permissions::handler("reset".to_string())
        .await
        .unwrap();
    // No more lying about clearing — the TUI dispatcher does that.
    assert!(output.contains("only effective inside the TUI"));
    assert!(output.contains("File-based rules"));
}

#[tokio::test]
async fn test_cost_handler() {
    let output = handlers::cost::handler(String::new()).await.unwrap();
    assert!(output.contains("Session cost"));
}

#[tokio::test]
async fn test_context_handler() {
    let output = handlers::context::handler(String::new()).await.unwrap();
    assert!(output.contains("active session runtime"));
    assert!(!output.contains("200,000"));
}

#[tokio::test]
async fn test_compact_handler_empty() {
    let output = handlers::compact::handler(String::new()).await.unwrap();
    // Sentinel always present so SDK / TUI can dispatch the request.
    assert!(output.starts_with(handlers::compact::COMPACT_SENTINEL));
    assert!(output.contains("Compacting"));
}

#[tokio::test]
async fn test_compact_handler_with_instructions() {
    let output = handlers::compact::handler("focus on the API changes".to_string())
        .await
        .unwrap();
    assert!(output.contains("focus on the API changes"));
}

#[tokio::test]
async fn test_mcp_handler_list() {
    let output = handlers::mcp::handler(String::new()).await.unwrap();
    assert!(output.contains("MCP"));
    assert!(output.contains("enable") || output.contains("disable"));
}

#[tokio::test]
async fn test_mcp_handler_enable() {
    let output = handlers::mcp::handler("enable my-server".to_string())
        .await
        .unwrap();
    assert!(output.contains("Enabling"));
    assert!(output.contains("my-server"));
}

#[tokio::test]
async fn test_plugin_handler_list() {
    let output = handlers::plugin::handler(String::new()).await.unwrap();
    assert!(output.contains("plugin") || output.contains("Plugin"));
    assert!(output.contains("install"));
}

#[tokio::test]
async fn test_plugin_handler_install() {
    let output = handlers::plugin::handler("install my-plugin".to_string())
        .await
        .unwrap();
    // The handler either installs successfully or reports already installed
    assert!(
        output.contains("nstall") || output.contains("my-plugin"),
        "unexpected: {output}"
    );
}

// Integration test: diff handler runs real git (if in a git repo)
#[tokio::test]
async fn test_diff_handler() {
    let output = handlers::diff::handler(String::new()).await.unwrap();
    // In a git repo, should produce some output (even if empty diff)
    assert!(!output.is_empty());
}

// Integration test: init handler checks for real files
#[tokio::test]
async fn test_init_handler_async() {
    let output = init_handler_async(String::new()).await.unwrap();
    assert!(output.contains("Initializing"));
}

// Integration test: doctor runs real checks
#[tokio::test]
async fn test_doctor_handler_async() {
    let output = doctor_handler_async(String::new()).await.unwrap();
    assert!(output.contains("diagnostics") || output.contains("Diagnostics"));
    assert!(output.contains("git") || output.contains("shell"));
}

#[test]
fn rename_handler_explicit_emits_sentinel_with_name() {
    let out = rename_handler("fix-login");
    assert!(out.starts_with(RENAME_SENTINEL));
    let parsed = parse_rename_sentinel(&out).expect("sentinel must parse");
    assert_eq!(parsed, ParsedRename::Explicit("fix-login".into()));
}

#[test]
fn rename_handler_explicit_trims_whitespace_around_name() {
    let out = rename_handler("   fix-login   ");
    let parsed = parse_rename_sentinel(&out).expect("sentinel must parse");
    assert_eq!(parsed, ParsedRename::Explicit("fix-login".into()));
}

#[test]
fn rename_handler_empty_emits_bare_sentinel_for_auto_generation() {
    // Empty / whitespace-only args = auto-generate path (Fast role).
    // Handler MUST emit the sentinel so the runner can dispatch — the
    // pre-refactor behaviour of returning a usage string suppressed
    // the trigger entirely.
    for empty in ["", "   ", "\n"] {
        let out = rename_handler(empty);
        let parsed = parse_rename_sentinel(&out)
            .unwrap_or_else(|| panic!("sentinel must parse for `{empty:?}`"));
        assert_eq!(parsed, ParsedRename::Auto);
    }
}

#[test]
fn parse_rename_sentinel_rejects_unrelated_text() {
    assert_eq!(parse_rename_sentinel("hello world"), None);
    assert_eq!(parse_rename_sentinel("__COCO_OTHER__ foo"), None);
}

#[test]
fn status_handler_emits_sentinel() {
    let out = status_extended_handler("");
    assert!(
        out.starts_with(STATUS_SENTINEL),
        "must lead with sentinel: {out}"
    );
    assert!(parse_status_sentinel(&out).is_some());
}

#[test]
fn parse_status_sentinel_rejects_unrelated_text() {
    assert!(parse_status_sentinel("__COCO_STATUS__\nbody").is_some());
    assert_eq!(parse_status_sentinel("hello"), None);
    assert_eq!(parse_status_sentinel("__COCO_OTHER__"), None);
}

#[test]
fn test_alias_lookups_in_extended() {
    let mut registry = CommandRegistry::new();
    register_extended_builtins(&mut registry);

    assert!(
        registry.get("ctx").is_none(),
        "/ctx alias removed; use canonical /context"
    );

    assert!(
        registry.get("continue").is_none(),
        "/continue alias removed; use canonical /resume"
    );

    let by_alias = registry.get("plugins");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "plugin");

    let by_alias = registry.get("marketplace");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "plugin");

    let by_alias = registry.get("allowed-tools");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "permissions");

    assert!(
        registry.get("checkpoint").is_none(),
        "/checkpoint alias removed; use canonical /rewind"
    );
    assert!(
        registry.get("undo").is_none(),
        "/undo alias removed; use canonical /rewind"
    );

    let by_alias = registry.get("quit");
    assert!(by_alias.is_some());
    assert_eq!(by_alias.unwrap().base.name, "exit");
}
