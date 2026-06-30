//! Unit tests for the TUI driver's pure helpers.
//!
//! `run_agent_driver` itself is an integration point (talks to model
//! runtimes, spawns tokio tasks, etc.) so we exercise only the
//! decomposed pure logic here.

#[cfg(test)]
mod agent_template_tests {
    use super::super::build_agent_template;
    use super::super::yaml_single_quote;
    use coco_types::AgentColorName;
    use pretty_assertions::assert_eq;

    #[test]
    fn yaml_single_quote_doubles_inner_apostrophes() {
        assert_eq!(yaml_single_quote("plain"), "'plain'");
        assert_eq!(yaml_single_quote("it's fine"), "'it''s fine'");
        // Backslashes pass through literally — YAML single-quoted
        // form treats backslash as a normal character.
        assert_eq!(yaml_single_quote("a\\b"), "'a\\b'");
    }

    #[test]
    fn template_emits_color_line_when_provided() {
        let body = build_agent_template("Plan", "Plans things.", Some(AgentColorName::Blue));
        assert!(body.contains("name: Plan"));
        assert!(body.contains("description: 'Plans things.'"));
        assert!(body.contains("color: blue"));
    }

    #[test]
    fn template_omits_color_line_when_palette_full() {
        let body = build_agent_template("Plan", "x", None);
        assert!(!body.contains("color:"));
    }

    #[test]
    fn template_round_trips_through_subagent_parser() {
        // Smoke: hand the wizard's emitted YAML to the live
        // frontmatter parser to confirm the result is loadable.
        // Catches accidental YAML syntax drift in the template
        // (especially around single-quote escaping of inputs that
        // contain apostrophes).
        let body = build_agent_template(
            "demo-agent",
            "Handles when 'edge' cases collide.",
            Some(AgentColorName::Green),
        );
        // The loader-side flow is two-step: parse the markdown to
        // split frontmatter+content, then validate the parsed map.
        // Mirror that here so the test exercises the same pipeline
        // a real agent file goes through.
        let parsed = coco_frontmatter::parse(&body);
        let path = std::path::Path::new("/virtual/demo-agent.md");
        let (def, errors) = coco_subagent::parse_agent_markdown(
            path,
            &parsed.content,
            &parsed.data,
            coco_types::AgentSource::UserSettings,
        )
        .expect("template must parse as a valid agent definition");
        assert!(
            errors.is_empty(),
            "template must parse without validation errors: {errors:?}"
        );
        assert_eq!(def.name, "demo-agent");
        assert_eq!(
            def.description.as_deref(),
            Some("Handles when 'edge' cases collide.")
        );
        assert_eq!(def.color, Some(AgentColorName::Green));
    }
}

#[cfg(test)]
mod goal_tests {
    use super::super::workspace_trust_rejected_from_env;

    #[test]
    fn workspace_trust_gate_only_rejects_explicit_zero() {
        assert!(workspace_trust_rejected_from_env(Some("0")));
        assert!(!workspace_trust_rejected_from_env(Some("1")));
        assert!(!workspace_trust_rejected_from_env(None));
    }

    #[test]
    fn active_goal_status_matches_non_interactive_goal_contract() {
        let mut goal = coco_types::ActiveGoal {
            condition: "finish the migration".to_string(),
            iterations: 0,
            set_at_ms: 100,
            tokens_at_start: 10,
            last_reason: None,
        };

        assert_eq!(
            coco_cli::goal_command::format_active_goal_status(&goal),
            "Goal active: finish the migration (not yet evaluated)"
        );

        goal.iterations = 2;
        goal.last_reason = Some(" tests still failing\nrerun needed ".to_string());
        assert_eq!(
            coco_cli::goal_command::format_active_goal_status(&goal),
            "Goal active: finish the migration (2 turns)\ntests still failing rerun needed"
        );
    }
}

use super::ActiveTurn;
use super::ActiveTurnDrain;
use super::PermissionsMutation;
use super::SentinelTrigger;
use super::add_dir_already_message;
use super::build_remote_model_change_reminder;
use super::build_system_message_from_push_kind;
use super::classify_sentinel_trigger;
use super::create_slash_metadata_message;
use super::dispatch_slash_command;
use super::drain_active_turn;
use super::format_slash_command_metadata;
use super::parse_editor_command;
use super::parse_permissions_mutation;
use super::parse_slash_command;
use super::session_plan_file_path;
use super::should_prompt_mode_bash_respond;
use super::should_trigger_title_gen;
use super::slash_unavailable_in_session_message;
use clap::Parser;
use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_tool_runtime::TurnAbortController;
use coco_tui::SystemPushKind;
use coco_types::CommandArgumentKind;
use coco_types::CommandBase;
use coco_types::CommandSafety;
use coco_types::CommandSource;
use coco_types::CommandType;
use coco_types::LocalCommandData;
use coco_types::ModelRole;
use coco_types::ProviderModelSelection;
use coco_types::TurnAbortReason;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Mutex;

#[test]
fn add_dir_already_message_distinguishes_current_added_and_nested() {
    let current = std::path::Path::new("/repo");
    let additional = vec![std::path::PathBuf::from("/opt/shared")];

    assert_eq!(
        add_dir_already_message(std::path::Path::new("/repo"), current, &additional).as_deref(),
        Some("/repo is already the current working directory.")
    );
    assert_eq!(
        add_dir_already_message(std::path::Path::new("/opt/shared"), current, &additional)
            .as_deref(),
        Some("/opt/shared is already added as a working directory.")
    );
    assert_eq!(
        add_dir_already_message(std::path::Path::new("/repo/src"), current, &additional).as_deref(),
        Some("/repo/src is already accessible within the current working directory /repo.")
    );
    assert_eq!(
        add_dir_already_message(
            std::path::Path::new("/opt/shared/tools"),
            current,
            &additional,
        )
        .as_deref(),
        Some(
            "/opt/shared/tools is already accessible within the additional working directory /opt/shared."
        )
    );
    assert!(
        add_dir_already_message(std::path::Path::new("/tmp/other"), current, &additional).is_none()
    );
}

#[test]
fn title_gen_fires_when_all_conditions_met() {
    assert!(should_trigger_title_gen(
        /*auto_title_enabled*/ true, /*already_attempted*/ false,
        /*fast_spec_present*/ true, /*plan_has_exited*/ true,
        /*plan_text_non_empty*/ true,
    ));
}

#[tokio::test]
async fn shutdown_drain_aborts_stuck_active_turn_after_timeout() {
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let dropped_for_task = dropped.clone();
    let task = tokio::spawn(async move {
        let _guard = DropFlag(dropped_for_task);
        std::future::pending::<()>().await;
    });
    let abort = TurnAbortController::new();
    let signal = abort.signal();
    let slot = Arc::new(Mutex::new(Some(ActiveTurn {
        id: uuid::Uuid::new_v4(),
        task,
        abort,
    })));

    drain_active_turn(
        &slot,
        ActiveTurnDrain::AbortAfter(Duration::from_millis(10)),
    )
    .await;

    assert!(slot.lock().await.is_none());
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(signal.reason(), Some(TurnAbortReason::SystemPreempt));
}

#[test]
fn title_gen_gated_off_by_setting() {
    // User hasn't opted in.
    assert!(!should_trigger_title_gen(false, false, true, true, true));
}

#[test]
fn title_gen_does_not_retry_after_first_attempt() {
    // Latch: once we've attempted, don't re-fire even if conditions still hold.
    assert!(!should_trigger_title_gen(true, true, true, true, true));
}

#[test]
fn title_gen_skipped_without_fast_model() {
    // User enabled auto_title but hasn't wired up a Fast role / the
    // `ANTHROPIC_API_KEY` fallback isn't available. Silent skip.
    assert!(!should_trigger_title_gen(true, false, false, true, true));
}

#[test]
fn title_gen_skipped_before_plan_exited() {
    // Model hasn't successfully exited plan mode yet this session.
    assert!(!should_trigger_title_gen(true, false, true, false, true));
}

#[test]
fn title_gen_skipped_with_empty_plan() {
    // ExitPlanMode ran against an empty plan file (e.g. model called
    // Exit before writing anything). No useful context to summarize.
    assert!(!should_trigger_title_gen(true, false, true, true, false));
}

#[test]
fn remote_model_change_reminder_is_remote_main_only() {
    let msg = build_remote_model_change_reminder(ModelRole::Main, "Claude Sonnet 4.6", true)
        .expect("remote main switch should emit reminder");
    let text = coco_messages::wrapping::extract_text_from_message(&msg);
    assert!(text.contains("<system-reminder>"));
    assert!(text.contains("The model for this session has been changed to Claude Sonnet 4.6."));
    assert!(text.contains("You are now running as Claude Sonnet 4.6."));

    assert!(
        build_remote_model_change_reminder(ModelRole::Main, "Claude Sonnet 4.6", false).is_none()
    );
    assert!(
        build_remote_model_change_reminder(ModelRole::Plan, "Claude Sonnet 4.6", true).is_none()
    );
}

#[test]
fn permission_retry_system_push_builds_permission_retry_message() {
    let msg = build_system_message_from_push_kind(SystemPushKind::PermissionRetry {
        tool_name: "Bash".into(),
        message: "Permission granted for: Bash. You may now retry this command if you would like."
            .into(),
    });

    match msg {
        coco_messages::Message::System(coco_messages::SystemMessage::PermissionRetry(retry)) => {
            assert_eq!(retry.tool_name, "Bash");
            assert_eq!(
                retry.message,
                "Permission granted for: Bash. You may now retry this command if you would like."
            );
        }
        other => panic!("expected PermissionRetry system message, got {other:?}"),
    }
}

#[test]
fn parse_slash_extracts_name_only() {
    assert_eq!(parse_slash_command("/help"), Some(("help", "")));
}

#[test]
fn parse_slash_splits_args() {
    assert_eq!(
        parse_slash_command("/commit focus on auth changes"),
        Some(("commit", "focus on auth changes"))
    );
}

#[test]
fn parse_slash_collapses_extra_whitespace() {
    // Single space after the name is the conventional separator;
    // additional whitespace is preserved as part of args (the
    // handlers themselves trim again).
    assert_eq!(
        parse_slash_command("/commit   spaced"),
        Some(("commit", "spaced"))
    );
}

#[test]
fn parse_slash_trims_outer_whitespace() {
    assert_eq!(parse_slash_command("   /diff   "), Some(("diff", "")));
}

#[test]
fn parse_slash_rejects_non_slash() {
    assert_eq!(parse_slash_command("hello world"), None);
}

#[test]
fn parse_slash_rejects_bare_slash() {
    assert_eq!(parse_slash_command("/"), None);
    assert_eq!(parse_slash_command("   /   "), None);
}

fn inactive_test_command_enabled() -> bool {
    false
}

fn inactive_test_command_handler(_args: &str) -> String {
    "handler should not run".to_string()
}

async fn build_runtime_with_registry(
    home: &TempDir,
    registry: coco_commands::CommandRegistry,
) -> Arc<crate::session_runtime::SessionRuntime> {
    build_runtime_with_registry_and_settings(home, registry, Settings::default()).await
}

async fn build_runtime_with_registry_and_settings(
    home: &TempDir,
    registry: coco_commands::CommandRegistry,
    settings_overrides: Settings,
) -> Arc<crate::session_runtime::SessionRuntime> {
    let settings = SettingsWithSource {
        merged: Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-opus-4-7".into(),
                })),
                ..Default::default()
            },
            available_models: settings_overrides.available_models,
            respond_to_bash_commands: settings_overrides.respond_to_bash_commands,
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = coco_config::build_runtime_config_with(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");
    let model_id = coco_cli::headless::resolve_main_model(&runtime_config).model_id;
    let cli = coco_cli::Cli::try_parse_from(["coco"]).expect("parse cli");

    crate::session_runtime::SessionRuntime::build(crate::session_runtime::SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        cwd: home.path().to_path_buf(),
        model_id,
        system_prompt: "test".to_string(),
        bypass_permissions_available: false,
        permission_mode: coco_types::PermissionMode::default(),
        model_runtimes: None,
        tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
        session_manager: Arc::new(coco_session::SessionManager::new(
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(registry))),
        skill_manager: Arc::new(coco_skills::SkillManager::new()),
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: false,
    })
    .await
    .expect("build runtime")
}

#[tokio::test]
async fn prompt_mode_bash_responds_by_default() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;

    assert!(should_prompt_mode_bash_respond(&runtime));
}

#[tokio::test]
async fn prompt_mode_bash_respects_respond_setting_false() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry_and_settings(
        &home,
        registry,
        Settings {
            respond_to_bash_commands: Some(false),
            ..Default::default()
        },
    )
    .await;

    assert!(!should_prompt_mode_bash_respond(&runtime));
}

#[tokio::test]
async fn model_slash_arg_rejects_unavailable_model() {
    let mut registry = coco_commands::CommandRegistry::new();
    coco_commands::register_extended_builtins(&mut registry);
    let home = TempDir::new().expect("tempdir");
    let runtime = build_runtime_with_registry_and_settings(
        &home,
        registry,
        Settings {
            available_models: Some(vec!["anthropic/claude-opus-4-7".to_string()]),
            ..Default::default()
        },
    )
    .await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);

    let outcome = dispatch_slash_command("model", "gpt5", &runtime, &tx).await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let event = rx.recv().await.expect("slash result event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            args,
            text,
        }) => {
            assert_eq!(name, "model");
            assert_eq!(args, "gpt5");
            assert!(text.contains("restricted by your organization's settings"));
            assert!(text.contains("Run /model to choose a different model"));
            assert!(text.contains("gpt-5-4"));
        }
        other => panic!("expected slash result, got {other:?}"),
    }
}

#[tokio::test]
async fn inactive_slash_command_emits_session_hint_without_running_handler() {
    let mut registry = coco_commands::CommandRegistry::new();
    registry.register(coco_commands::RegisteredCommand {
        base: CommandBase {
            name: "blocked".to_string(),
            description: "inactive command".to_string(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: false,
            loaded_from: Some(CommandSource::Builtin),
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(LocalCommandData {
            handler: "blocked".to_string(),
        }),
        handler: Some(Arc::new(coco_commands::BuiltinCommand::new(
            "blocked",
            inactive_test_command_handler,
        ))),
        is_enabled: Some(inactive_test_command_enabled),
    });
    let home = TempDir::new().expect("tempdir");
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);

    let outcome = dispatch_slash_command("blocked", "arg", &runtime, &tx).await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let event = rx.recv().await.expect("slash result event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            args,
            text,
        }) => {
            assert_eq!(name, "blocked");
            assert_eq!(args, "arg");
            assert_eq!(text, slash_unavailable_in_session_message("blocked"));
            assert_ne!(text, "handler should not run");
        }
        other => panic!("expected slash result, got {other:?}"),
    }
}

#[test]
fn slash_prompt_metadata_matches_ts_shape() {
    assert_eq!(
        format_slash_command_metadata("simplify", "focus on tests"),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>\n\
         <command-args>focus on tests</command-args>"
    );
}

#[test]
fn slash_prompt_metadata_omits_empty_args() {
    assert_eq!(
        format_slash_command_metadata("simplify", ""),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>"
    );
}

#[test]
fn slash_prompt_metadata_message_has_distinct_identity_and_kind() {
    let metadata = format_slash_command_metadata("simplify", "focus");
    let message = create_slash_metadata_message(&metadata);
    let coco_messages::Message::Attachment(attachment) = message else {
        panic!("slash metadata should be an attachment");
    };
    assert_eq!(
        attachment.kind,
        coco_types::AttachmentKind::SlashCommandMetadata
    );
    assert_ne!(attachment.uuid, uuid::Uuid::nil());
}

#[test]
fn session_plan_file_path_uses_runtime_plan_directory_setting() {
    let config_home = tempfile::tempdir().expect("config home");
    let project = tempfile::tempdir().expect("project");
    let path = session_plan_file_path(
        config_home.path(),
        Some(project.path()),
        Some("plans"),
        "session-1",
    );

    assert!(path.starts_with(project.path().canonicalize().unwrap().join("plans")));
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("md"));
}

#[test]
fn parse_editor_command_splits_quoted_args() {
    let (program, args) =
        parse_editor_command("code --wait --reuse-window 'memory file.md'").expect("parsed");
    assert_eq!(program, "code");
    assert_eq!(args, vec!["--wait", "--reuse-window", "memory file.md"]);
}

#[test]
fn parse_editor_command_rejects_unbalanced_quotes() {
    let err = parse_editor_command("code 'unterminated").expect_err("should reject");
    assert!(err.contains("failed to parse editor command"));
}

// `classify_sentinel_trigger` — decides whether a registry handler's
// Text output is actually a request to fire a real feature (compact /
// dream / summary). Wrong classification means the user's `/compact`
// would silently print sentinel garbage instead of triggering compaction.

#[test]
fn classify_sentinel_compact_no_args() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("{COMPACT_SENTINEL} \nCompacting conversation…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: None
        })
    );
}

#[test]
fn classify_sentinel_compact_with_instructions() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("{COMPACT_SENTINEL} focus on auth\nCompacting…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: Some("focus on auth".to_string()),
        })
    );
}

#[test]
fn classify_sentinel_compact_whitespace_only_args_treated_as_none() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    // The handler emits "{SENTINEL}  \n" when args is whitespace; trim
    // should fold that back to None so the engine doesn't see an empty
    // custom_instructions string.
    let text = format!("{COMPACT_SENTINEL}    \nCompacting…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: None
        })
    );
}

#[test]
fn classify_sentinel_dream() {
    use coco_commands::handlers::dream::DREAM_SENTINEL;
    let text = format!("{DREAM_SENTINEL} \nKAIROS dream consolidation…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Dream)
    );
}

#[test]
fn classify_sentinel_btw() {
    use coco_commands::handlers::btw::BTW_SENTINEL;
    let text = format!("{BTW_SENTINEL} how does caching work?");
    assert!(matches!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Btw { request }) if request.question == "how does caching work?"
    ));
}

#[test]
fn classify_sentinel_btw_empty_question_is_none() {
    use coco_commands::handlers::btw::BTW_SENTINEL;
    let text = format!("{BTW_SENTINEL} ");
    assert_eq!(classify_sentinel_trigger(&text), None);
}

#[test]
fn first_user_prompt_title_uses_first_line_truncated() {
    use coco_messages::create_user_message;
    let msgs = vec![create_user_message("Build the auth flow\nmore detail")];
    assert_eq!(
        super::first_user_prompt_title(&msgs).as_deref(),
        Some("Build the auth flow")
    );
}

#[test]
fn classify_sentinel_summary() {
    use coco_commands::handlers::summary::SUMMARY_SENTINEL;
    let text = format!("{SUMMARY_SENTINEL} \nWriting session memory…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Summary)
    );
}

#[test]
fn classify_sentinel_plain_text_returns_none() {
    // The vast majority of handler outputs — anything not starting with
    // a sentinel — must classify as None so the dispatcher renders them
    // verbatim in the transcript.
    assert_eq!(classify_sentinel_trigger(""), None);
    assert_eq!(classify_sentinel_trigger("Hello, world"), None);
    assert_eq!(
        classify_sentinel_trigger("## Permission Rules\n\nNo rules"),
        None
    );
}

#[test]
fn classify_sentinel_does_not_match_substring() {
    // Sentinels must be at the *start*; a sentinel embedded in body text
    // (e.g. echoed inside an explanation) must not trigger.
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("Here is the sentinel: {COMPACT_SENTINEL}");
    assert_eq!(classify_sentinel_trigger(&text), None);
}

// `parse_permissions_mutation` — distinguishes the read-only / list
// path (None, falls through to registry) from the three mutating
// subcommands the TUI dispatcher actually applies to engine_config.

#[test]
fn parse_permissions_reset() {
    assert_eq!(
        parse_permissions_mutation("reset"),
        Some(PermissionsMutation::Reset)
    );
    assert_eq!(
        parse_permissions_mutation("  reset  "),
        Some(PermissionsMutation::Reset)
    );
}

#[test]
fn parse_permissions_allow() {
    assert_eq!(
        parse_permissions_mutation("allow Bash"),
        Some(PermissionsMutation::Allow("Bash".to_string()))
    );
    assert_eq!(
        parse_permissions_mutation("allow mcp__server__tool"),
        Some(PermissionsMutation::Allow("mcp__server__tool".to_string()))
    );
}

#[test]
fn parse_permissions_deny() {
    assert_eq!(
        parse_permissions_mutation("deny Write"),
        Some(PermissionsMutation::Deny("Write".to_string()))
    );
}

#[test]
fn parse_permissions_list_falls_through_to_registry() {
    // The read-only paths return None so the dispatcher hands off to
    // the registry handler (which reads settings.json and renders).
    assert_eq!(parse_permissions_mutation(""), None);
    assert_eq!(parse_permissions_mutation("list"), None);
    assert_eq!(parse_permissions_mutation("  "), None);
}

#[test]
fn parse_permissions_allow_without_tool_is_none() {
    // `allow ` with no tool name must fall through (the dispatcher then
    // emits a usage hint) — never let an empty-string tool reach
    // engine_config.allow_rules.
    assert_eq!(parse_permissions_mutation("allow"), None);
    assert_eq!(parse_permissions_mutation("allow "), None);
    assert_eq!(parse_permissions_mutation("allow   "), None);
}

#[test]
fn parse_permissions_deny_without_tool_is_none() {
    assert_eq!(parse_permissions_mutation("deny"), None);
    assert_eq!(parse_permissions_mutation("deny "), None);
}

#[test]
fn parse_permissions_unknown_subcommand_is_none() {
    // Unknown words pass through to the registry handler, which renders
    // its own "Unknown permissions subcommand" error.
    assert_eq!(parse_permissions_mutation("foobar"), None);
    assert_eq!(parse_permissions_mutation("revoke Bash"), None);
}

#[test]
fn title_gen_exhaustive_truth_table() {
    // Exhaustive check: the gate returns true for EXACTLY the all-true
    // combination and false for every other combination. Catches any
    // future refactor that accidentally makes one condition optional.
    for auto in [false, true] {
        for already in [false, true] {
            for spec in [false, true] {
                for exited in [false, true] {
                    for plan in [false, true] {
                        let result = should_trigger_title_gen(auto, already, spec, exited, plan);
                        let expected = auto && !already && spec && exited && plan;
                        assert_eq!(
                            result, expected,
                            "auto={auto} already={already} spec={spec} exited={exited} plan={plan}"
                        );
                    }
                }
            }
        }
    }
}

// ── /plan dispatch ──
//
// `dispatch_plan` itself talks to a `SessionRuntime` and is exercised
// by integration tests. The rule "fire a query if and only if `args`
// is non-empty AND not 'open'" is encoded in
// `plan_command_query_after_flip` as a pure helper so we cover that
// regression-prone branch without spinning up the runtime.

use super::plan_command_query_after_flip;

#[test]
fn plan_query_after_flip_fires_for_real_description() {
    assert_eq!(
        plan_command_query_after_flip("refactor the auth flow"),
        Some("refactor the auth flow")
    );
}

#[test]
fn plan_query_after_flip_trims_whitespace() {
    assert_eq!(
        plan_command_query_after_flip("   refactor   "),
        Some("refactor")
    );
}

#[test]
fn plan_query_after_flip_skips_bare_plan() {
    // Bare `/plan` (empty args) calls `onDone('Enabled plan mode')`
    // WITHOUT `shouldQuery`.
    assert_eq!(plan_command_query_after_flip(""), None);
    assert_eq!(plan_command_query_after_flip("   "), None);
}

#[test]
fn plan_query_after_flip_skips_open_subcommand() {
    // `description !== 'open'` filter — `/plan open` opens an editor,
    // never fires a query.
    assert_eq!(plan_command_query_after_flip("open"), None);
    assert_eq!(plan_command_query_after_flip("  open  "), None);
}

#[test]
fn plan_query_after_flip_open_substring_still_queries() {
    // Only the bare token "open" suppresses the query — descriptions
    // that happen to contain it must still query.
    assert_eq!(
        plan_command_query_after_flip("open the door"),
        Some("open the door")
    );
}

mod truncate_output_tests {
    use super::super::truncate_output;
    use pretty_assertions::assert_eq;

    #[test]
    fn short_text_passes_through() {
        assert_eq!(truncate_output("hello".into(), 100, 10), "hello");
    }

    #[test]
    fn caps_at_line_count_with_marker() {
        let text = (0..15)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate_output(text, 10_000, 5);
        let lines = out.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[4], "line 4");
        assert_eq!(lines[5], "… (truncated)");
    }

    #[test]
    fn caps_at_byte_budget() {
        let long = "x".repeat(500);
        let out = truncate_output(long, 50, 1000);
        assert!(out.starts_with(&"x".repeat(50)));
        assert!(out.ends_with("(truncated)"));
    }

    #[test]
    fn preserves_utf8_boundaries_when_cut() {
        // Each 🚀 is 4 bytes; 60 chars × 4 = 240 bytes. The byte cut
        // must land on a 4-byte boundary so the string stays valid
        // UTF-8 (`.chars().count()` panics on a malformed slice).
        let rocket_run: String = "🚀".repeat(60);
        let out = truncate_output(rocket_run, 100, 1000);
        let _ = out.chars().count();
        assert!(out.ends_with("(truncated)"));
    }
}

mod turn_done_guard_tests {
    use super::super::TurnDoneGuard;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn fires_on_normal_scope_exit() {
        let (tx, mut rx) = mpsc::channel::<uuid::Uuid>(4);
        let id = uuid::Uuid::new_v4();
        {
            let _guard = TurnDoneGuard {
                turn_id: id,
                tx: tx.clone(),
            };
        }
        assert_eq!(rx.recv().await, Some(id));
    }

    #[tokio::test]
    async fn fires_on_panic_unwind_inside_spawn() {
        // The bug we're guarding against: a spawned turn task panics
        // before the original tail `turn_done_tx.send(...)` runs, so
        // the completion signal never fires and `active_turn` stays
        // locked. Drop runs during unwind, so the guard must signal
        // even on panic.
        let (tx, mut rx) = mpsc::channel::<uuid::Uuid>(4);
        let id = uuid::Uuid::new_v4();
        let tx_t = tx.clone();
        let prior_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let handle = tokio::spawn(async move {
            let _guard = TurnDoneGuard {
                turn_id: id,
                tx: tx_t,
            };
            panic!("intentional turn-task panic for test");
        });
        let res = handle.await;
        std::panic::set_hook(prior_hook);
        assert!(res.is_err(), "spawned task should have surfaced JoinError");
        assert_eq!(rx.recv().await, Some(id));
    }
}
