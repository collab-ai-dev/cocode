use clap::Parser;

use super::Cli;

/// PR-E3 flags should parse without colliding with pre-existing flags.
/// Verifies every new flag round-trips through clap parsing.
#[test]
fn parses_all_pr_e3_flags() {
    let args = [
        "coco",
        "--input-format",
        "stream-json",
        "--json-schema",
        "/tmp/schema.json",
        "--replay-user-messages",
        "--include-hook-events",
        "--include-partial-messages",
        "--thinking",
        "adaptive",
        "--max-thinking-tokens",
        "8192",
        "--append-system-prompt-file",
        "/tmp/extra.md",
        "--strict-mcp-config",
        "--setting-sources",
        "user,project",
        "--fork-session",
        "--betas",
        "prompt-caching-2024-07-31",
        "--session-id",
        "11111111-2222-3333-4444-555555555555",
        "--permission-prompt-tool",
        "mcp__approval__prompt",
    ];
    let cli = Cli::try_parse_from(args).expect("parse pr-e3 flags");

    assert_eq!(cli.input_format.as_deref(), Some("stream-json"));
    assert_eq!(cli.json_schema.as_deref(), Some("/tmp/schema.json"));
    assert!(cli.replay_user_messages);
    assert!(cli.include_hook_events);
    assert!(cli.include_partial_messages);
    assert_eq!(cli.thinking.as_deref(), Some("adaptive"));
    assert_eq!(cli.max_thinking_tokens, Some(8192));
    assert_eq!(
        cli.append_system_prompt_file.as_deref(),
        Some("/tmp/extra.md")
    );
    assert!(cli.strict_mcp_config);
    assert_eq!(cli.setting_sources.as_deref(), Some("user,project"));
    assert!(cli.fork_session);
    assert_eq!(cli.betas.as_deref(), Some("prompt-caching-2024-07-31"));
    assert_eq!(
        cli.session_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    assert_eq!(
        cli.permission_prompt_tool.as_deref(),
        Some("mcp__approval__prompt")
    );
}

/// Existing flags must still parse; new additions must not change existing
/// defaults or argument parsing for pre-PR-E3 flags.
#[test]
fn pr_e3_defaults_leave_existing_flags_untouched() {
    let cli = Cli::try_parse_from(["coco"]).expect("parse no-arg");
    assert!(cli.input_format.is_none());
    assert!(cli.json_schema.is_none());
    assert!(!cli.replay_user_messages);
    assert!(!cli.include_hook_events);
    assert!(!cli.include_partial_messages);
    assert!(cli.thinking.is_none());
    assert!(cli.max_thinking_tokens.is_none());
    assert!(cli.append_system_prompt_file.is_none());
    assert!(!cli.strict_mcp_config);
    assert!(cli.setting_sources.is_none());
    assert!(!cli.fork_session);
    assert!(cli.betas.is_none());
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
    assert!(cli.permission_prompt_tool.is_none());
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
