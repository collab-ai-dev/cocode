use std::collections::BTreeMap;
use std::collections::BTreeSet;

use clap::CommandFactory;
use clap::Parser;

use super::Cli;

#[test]
fn parses_supported_pr_e3_flags() {
    let args = [
        "coco",
        "--json-schema",
        "/tmp/schema.json",
        "--include-hook-events",
        "--append-system-prompt-file",
        "/tmp/extra.md",
        "--setting-sources",
        "user,project",
        "--fork-session",
        "--session-id",
        "11111111-2222-3333-4444-555555555555",
    ];
    let cli = Cli::try_parse_from(args).expect("parse pr-e3 flags");

    assert_eq!(cli.json_schema.as_deref(), Some("/tmp/schema.json"));
    assert!(cli.include_hook_events);
    assert_eq!(
        cli.append_system_prompt_file.as_deref(),
        Some("/tmp/extra.md")
    );
    assert_eq!(cli.setting_sources.as_deref(), Some("user,project"));
    assert!(cli.fork_session);
    assert_eq!(
        cli.session_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
}

#[test]
fn top_level_flags_have_documented_consumers() {
    let documented_consumers = BTreeMap::from([
        ("add-dir", "AgentHostOptions -> runtime allowed roots"),
        (
            "allow-dangerously-skip-permissions",
            "AgentHostOptions -> runtime permission policy",
        ),
        ("allowed-tools", "AgentHostOptions -> tool filter"),
        (
            "append-system-prompt",
            "AgentHostOptions -> runtime prompt config",
        ),
        (
            "append-system-prompt-file",
            "AgentHostOptions -> runtime prompt config",
        ),
        ("bare", "main startup env policy"),
        ("continue-session", "AgentHostOptions -> resume resolver"),
        ("cwd", "AgentHostOptions -> runtime cwd"),
        (
            "dangerously-skip-permissions",
            "AgentHostOptions -> runtime permission policy",
        ),
        ("disallowed-tools", "AgentHostOptions -> tool filter"),
        ("event-hub-url", "AgentHostOptions -> Event Hub connector"),
        ("fallback-model", "AgentHostOptions -> model fallback chain"),
        ("fork-session", "AgentHostOptions -> resume resolver"),
        ("hub-port", "embedded_hub startup policy"),
        (
            "include-hook-events",
            "AgentHostOptions -> query event stream",
        ),
        ("json-schema", "AgentHostOptions -> structured output tool"),
        ("log-file", "tracing_init"),
        ("log-format", "tracing_init"),
        ("log-level", "tracing_init"),
        ("log-location", "tracing_init"),
        ("log-stderr", "tracing_init"),
        ("log-timezone", "tracing_init"),
        ("max-tokens", "AgentHostOptions -> query config"),
        ("max-turns", "AgentHostOptions -> query config / SDK host"),
        ("models.main", "AgentHostOptions -> model resolver"),
        (
            "no-session-persistence",
            "ExecutionPlan validation + AgentHostOptions",
        ),
        ("non-interactive", "ExecutionPlan mode selection"),
        ("permission-mode", "AgentHostOptions -> permission policy"),
        (
            "plan-mode-instructions",
            "ExecutionPlan validation + AgentHostOptions",
        ),
        ("prompt", "ExecutionPlan/headless prompt + AgentHostOptions"),
        ("resume", "AgentHostOptions -> resume resolver"),
        ("serve-hub", "embedded_hub startup policy"),
        ("session-id", "AgentHostOptions -> session id override"),
        ("setting-sources", "AgentHostOptions -> settings loader"),
        ("settings", "AgentHostOptions/tracing settings loader"),
        ("system-prompt", "AgentHostOptions -> runtime prompt config"),
    ]);
    let actual_flags: BTreeSet<String> = Cli::command()
        .get_arguments()
        .filter_map(|arg| arg.get_long().map(ToOwned::to_owned))
        .collect();
    let expected_flags: BTreeSet<String> = documented_consumers
        .keys()
        .map(|flag| (*flag).to_string())
        .collect();

    assert_eq!(
        actual_flags, expected_flags,
        "every accepted top-level flag must have an explicit consumer or plan policy"
    );
}

#[test]
fn parses_event_hub_flags() {
    let cli = Cli::try_parse_from(["coco", "--serve-hub", "--hub-port", "0"])
        .expect("parse serve-hub flags");

    assert!(cli.serve_hub);
    assert_eq!(cli.hub_port, 0);
    assert!(cli.event_hub_url.is_none());
}

#[test]
fn event_hub_url_conflicts_with_serve_hub() {
    let Err(err) = Cli::try_parse_from([
        "coco",
        "--serve-hub",
        "--event-hub-url",
        "ws://127.0.0.1:8731/v1/connect",
    ]) else {
        panic!("embedded and external hub endpoints must be mutually exclusive");
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn removed_global_noop_flags_are_rejected() {
    for flag in [
        "--no-tui",
        "--json",
        "--debug",
        "--verbose",
        "--bg",
        "--background",
        "--thinking-budget",
        "--mcp-config",
        "--output-format",
        "--effort",
        "--worktree",
        "--name",
        "--agent",
        "--max-budget-usd",
        "--init-only",
        "--input-format",
        "--replay-user-messages",
        "--include-partial-messages",
        "--thinking",
        "--max-thinking-tokens",
        "--strict-mcp-config",
        "--betas",
        "--permission-prompt-tool",
    ] {
        let Err(err) = Cli::try_parse_from(["coco", flag]) else {
            panic!("{flag} should not be accepted as a global no-op flag");
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

#[test]
fn removed_placeholder_subcommands_are_rejected() {
    for args in [
        &["coco", "daemon"][..],
        &["coco", "logs", "session-id"][..],
        &["coco", "attach", "session-id"][..],
        &["coco", "kill", "session-id"][..],
        &["coco", "remote-control"][..],
        &["coco", "rc"][..],
        &["coco", "bridge"][..],
        &["coco", "sync"][..],
        &["coco", "upgrade"][..],
        &["coco", "usage"][..],
    ] {
        let Err(err) = Cli::try_parse_from(args) else {
            panic!("{args:?} should not be accepted as a placeholder subcommand");
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }
}

#[test]
fn hub_port_requires_serve_hub() {
    let Err(err) = Cli::try_parse_from(["coco", "--hub-port", "0"]) else {
        panic!("hub-port should only apply to serve-hub");
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

/// Existing flags must still parse; new additions must not change existing
/// defaults or argument parsing for pre-PR-E3 flags.
#[test]
fn pr_e3_defaults_leave_existing_flags_untouched() {
    let cli = Cli::try_parse_from(["coco"]).expect("parse no-arg");
    assert!(cli.json_schema.is_none());
    assert!(!cli.include_hook_events);
    assert!(cli.append_system_prompt_file.is_none());
    assert!(cli.setting_sources.is_none());
    assert!(!cli.fork_session);
    assert!(cli.session_id.is_none());
    assert!(
        cli.fallback_model.is_empty(),
        "no-arg invocation must leave fallback_model empty"
    );
}

#[test]
fn parses_repeated_fallback_model_flags_into_ordered_vec() {
    let cli = Cli::try_parse_from([
        "coco",
        "--fallback-model",
        "anthropic/claude-sonnet-4-6",
        "--fallback-model",
        "openai/gpt-5",
        "--fallback-model",
        "google/gemini-2.5-pro",
    ])
    .expect("parse repeated fallback flags");
    assert_eq!(
        cli.fallback_model,
        vec![
            "anthropic/claude-sonnet-4-6".to_string(),
            "openai/gpt-5".to_string(),
            "google/gemini-2.5-pro".to_string(),
        ],
        "flag order must be preserved for chain priority",
    );
}

#[test]
fn parses_single_fallback_model_flag_as_one_tier_chain() {
    // Legacy usage: a single `--fallback-model` remains a valid
    // one-tier chain, preserving existing muscle memory.
    let cli = Cli::try_parse_from(["coco", "--fallback-model", "anthropic/claude-sonnet-4-6"])
        .expect("parse single fallback flag");
    assert_eq!(cli.fallback_model, vec!["anthropic/claude-sonnet-4-6"]);
}

#[test]
fn parses_models_main_flag() {
    let cli = Cli::try_parse_from(["coco", "--models.main", "openai/gpt-5-5"])
        .expect("parse models.main flag");
    assert_eq!(cli.models_main.as_deref(), Some("openai/gpt-5-5"));
}

#[test]
fn does_not_accept_legacy_model_flag() {
    let Err(err) = Cli::try_parse_from(["coco", "--model", "openai/gpt-5-5"]) else {
        panic!("legacy --model flag must not parse");
    };
    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn parses_resume_short_flag() {
    let cli = Cli::try_parse_from(["coco", "-r", "auth-refactor"]).expect("parse -r resume");
    assert_eq!(cli.resume.as_deref(), Some("auth-refactor"));
}

#[test]
fn resume_does_not_accept_restore_alias() {
    let Err(err) = Cli::try_parse_from(["coco", "--restore", "auth-refactor"]) else {
        panic!("session resume must use --resume/-r; --restore is not a Claude Code CLI flag");
    };
    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
}
