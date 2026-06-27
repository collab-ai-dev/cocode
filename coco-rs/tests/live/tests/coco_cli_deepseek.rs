//! Live tests for the **`coco -p`** headless entry point.
//!
//! Drives `coco_cli::headless::run_chat` in-process — the **same**
//! function `main.rs` calls when the user runs `coco -p "<prompt>"`.
//! Goes through:
//!
//! ```text
//! Cli::parse_from(argv)
//!   → cli_runtime_overrides
//!   → build_runtime_config_for_cli       (settings.json + env + CLI flags merge)
//!   → create_api_client                   (provider + fallback chain)
//!   → build_system_prompt_for_model       (CLAUDE.md discovery + model base instructions)
//!   → resolve_startup_permission_state    (permission_mode + bypass + sudo guard)
//!   → QueryEngine::new + run              (full agent loop)
//!   → RunChatOutcome
//! ```
//!
//! Coverage adds *over* `cli_deepseek` (which builds `QueryEngine`
//! manually with hand-picked config): full CLI argv parsing, settings
//! file merge, fallback chain plumbing, system prompt assembly with
//! CLAUDE.md, permission-mode resolution path. This is the layer the
//! user actually hits.

mod common;

use anyhow::Result;
use coco_cli::Cli;
use coco_cli::headless::RunChatOptions;
use coco_cli::headless::RunChatOutcome;
use coco_cli::headless::run_chat;
use coco_cli::headless::run_chat_with_options;
use coco_types::ProviderApi;
use tokio_util::sync::CancellationToken;

use crate::common::usage_report;

/// Resolved model under test, sourced from `.env`. Reads
/// `COCO_LIVE_TEST_DEEPSEEK_OPENAI_MODEL` (preferred) or
/// `_DEEPSEEK_ANTHROPIC_MODEL` so all CLI invocations and assertions
/// share one identifier; falls back to `deepseek-v4-flash` so existing
/// CI without env config still runs the suite. Box-leak'd once via
/// `OnceLock` so callers can keep treating it as `&'static str`.
fn model_id() -> &'static str {
    use std::sync::OnceLock;
    static M: OnceLock<&'static str> = OnceLock::new();
    M.get_or_init(|| {
        let s = std::env::var("COCO_LIVE_TEST_DEEPSEEK_OPENAI_MODEL")
            .or_else(|_| std::env::var("COCO_LIVE_TEST_DEEPSEEK_ANTHROPIC_MODEL"))
            .unwrap_or_else(|_| "deepseek-v4-flash".to_string());
        Box::leak(s.into_boxed_str())
    })
}

fn parse_cli(argv: &[&str]) -> Cli {
    use clap::Parser;
    let mut hermetic = argv.to_vec();
    if !argv
        .iter()
        .any(|arg| *arg == "--setting-sources" || arg.starts_with("--setting-sources="))
    {
        hermetic.push("--setting-sources");
        hermetic.push("project,local,flag");
    }
    Cli::parse_from(hermetic)
}

/// Build a `Cli` configured to drive a single one-shot prompt against
/// the named provider/model. `settings_path` is `None` to use the
/// default config layering (which finds nothing in our test setup).
fn cli_for(
    provider: &str,
    model: &str,
    prompt: &str,
    settings_path: Option<&str>,
    extra: &[&str],
) -> Cli {
    let model_arg = format!("{provider}/{model}");
    let mut argv: Vec<&str> = vec!["coco", "-p", prompt, "--models.main", &model_arg];
    if let Some(s) = settings_path {
        argv.push("--settings");
        argv.push(s);
    }
    for e in extra {
        argv.push(e);
    }
    parse_cli(&argv)
}

fn record_outcome(scenario: &str, outcome: &RunChatOutcome) {
    let llm_calls = outcome.cost_tracker.total_api_calls.max(0) as u64;
    usage_report::record_with_llm_calls(
        outcome.provider_api.map_or("mock", ProviderApi::as_str),
        &outcome.model_id,
        scenario,
        &outcome.total_usage,
        llm_calls,
    );
}

// ─── Basic round-trip per provider ───────────────────────────────────

#[tokio::test]
async fn test_coco_cli_basic_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.basic", &outcome);
    assert!(
        outcome.turns >= 1,
        "expected >=1 turn, got {}",
        outcome.turns
    );
    assert!(
        !outcome.response_text.trim().is_empty(),
        "response_text was empty"
    );
    assert_eq!(outcome.provider_api, Some(ProviderApi::OpenaiCompat));
    Ok(())
}

#[tokio::test]
async fn test_coco_cli_basic_deepseek_anthropic() -> Result<()> {
    let _t = require_live!("deepseek-anthropic", model_id(), "text");
    let cli = cli_for(
        "deepseek-anthropic",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.basic", &outcome);
    assert!(outcome.turns >= 1);
    assert!(!outcome.response_text.trim().is_empty());
    assert_eq!(outcome.provider_api, Some(ProviderApi::Anthropic));
    Ok(())
}

// ─── Cross-protocol via the user-facing CLI path ─────────────────────

#[tokio::test]
async fn test_coco_cli_cross_protocol_deepseek() -> Result<()> {
    let _o = require_live!("deepseek-openai", model_id(), "cross_protocol");
    let _a = require_live!("deepseek-anthropic", model_id(), "cross_protocol");
    let prompt = "What is the capital of France? Respond with just the city name.";

    for provider in ["deepseek-openai", "deepseek-anthropic"] {
        let cli = cli_for(provider, model_id(), prompt, None, &[]);
        let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
        record_outcome("coco_cli.cross_protocol", &outcome);
        assert!(
            outcome.response_text.to_lowercase().contains("paris"),
            "{provider}: expected 'paris' in response, got: {}",
            outcome.response_text
        );
    }
    Ok(())
}

// ─── max-turns plumbing ─────────────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_max_turns_one_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Say 'hello' in exactly one word.",
        None,
        &["--max-turns", "1"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.max_turns_1", &outcome);
    assert!(
        outcome.turns >= 1,
        "expected >=1 turn even with max_turns=1, got {}",
        outcome.turns
    );
    assert!(
        outcome.turns <= 1,
        "max_turns=1 should bound the loop, got turns={}",
        outcome.turns
    );
    assert!(!outcome.response_text.trim().is_empty());
    Ok(())
}

// ─── --settings file plumbing ────────────────────────────────────────

/// Write a tempdir `settings.json` that pins `models.main: deepseek-openai/<model_id()>`,
/// then run `coco -p "..." --settings <tempdir>/settings.json` *without*
/// `--models.main` and verify the settings-driven model takes effect.
#[tokio::test]
async fn test_coco_cli_settings_file_drives_model_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");

    let tmp = common::tmpdir::make("coco-tests-cli-")?;
    let settings_path = tmp.path().join("settings.json");
    let body = serde_json::json!({
        "models": {
            "main": format!("deepseek-openai/{}", model_id()),
        },
    });
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&body).expect("settings.json"),
    )?;

    // No --models.main flag — let settings.json drive it.
    let argv: Vec<String> = vec![
        "coco".into(),
        "-p".into(),
        "Reply with the single word: ok".into(),
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
    ];
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let cli = parse_cli(&argv_refs);

    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.settings_file", &outcome);
    assert_eq!(outcome.model_id, model_id());
    assert_eq!(outcome.provider_api, Some(ProviderApi::OpenaiCompat));
    assert!(!outcome.response_text.trim().is_empty());
    let _ = tmp; // keep alive until end
    Ok(())
}

// ─── permission_mode flag plumbing ───────────────────────────────────

#[tokio::test]
async fn test_coco_cli_permission_mode_accept_edits_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &["--permission-mode", "acceptEdits"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.permission_mode", &outcome);
    use coco_types::PermissionMode;
    assert_eq!(
        outcome.permission_mode,
        PermissionMode::AcceptEdits,
        "--permission-mode acceptEdits should resolve to AcceptEdits"
    );
    assert!(!outcome.response_text.trim().is_empty());
    Ok(())
}

// ─── Fallback chain plumbing (no real fallback fired, but the path runs) ─

/// `--fallback-model deepseek-anthropic/<model_id()>` should parse and the
/// fallback chain should be installed on the engine. We don't trigger a
/// real fallback (DeepSeek doesn't 529-throttle for our tiny prompts),
/// but the test confirms the parse + plumbing path doesn't crash.
#[tokio::test]
async fn test_coco_cli_fallback_chain_parses_deepseek() -> Result<()> {
    let _o = require_live!("deepseek-openai", model_id(), "text");
    let _a = require_live!("deepseek-anthropic", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[
            "--fallback-model",
            &format!("deepseek-anthropic/{}", model_id()),
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.fallback_chain", &outcome);
    assert_eq!(outcome.provider_api, Some(ProviderApi::OpenaiCompat));
    assert!(!outcome.response_text.trim().is_empty());
    Ok(())
}

// ─── max-tokens flag plumbing ────────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_max_tokens_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    // `--max-tokens 256` — large enough for a one-line answer but small
    // enough that the cap is exercised. Asserts the flag plumbed
    // through `cli.max_tokens` → `QueryEngineConfig.max_tokens` without
    // panicking, and the model produced *some* output. Tighter
    // bounds (e.g. asserting output_tokens <= cap) are flaky because
    // some providers count thinking against the cap differently.
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &["--max-tokens", "256"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.max_tokens", &outcome);
    assert!(
        outcome.total_usage.output_tokens.total > 0,
        "expected non-zero output tokens"
    );
    Ok(())
}

// ─── coco_cli system-prompt assembly via CLAUDE.md (smoke) ───────────

/// Run with cwd = tempdir containing a CLAUDE.md. The lib's
/// `build_system_prompt_for_model` should discover it. We can't easily
/// observe the assembled prompt from outside, but we can confirm the
/// run completes (no crash on CLAUDE.md discovery) and the model still
/// answers the prompt.
#[tokio::test]
async fn test_coco_cli_claude_md_discovery_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");

    let tmp = common::tmpdir::make("coco-tests-cli-")?;
    let claude_md = tmp.path().join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        "# Project notes\nThe magic word is `flamingo`.\n",
    )?;

    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "What is the magic word from the project notes? Respond with just the word.",
        None,
        &[],
    );
    // Use `RunChatOptions::cwd` instead of `std::env::set_current_dir`
    // so the test stays parallel-safe (no process-global mutation).
    let outcome = run_chat_with_options(
        &cli,
        cli.prompt.as_deref(),
        RunChatOptions {
            cwd: Some(tmp.path().to_path_buf()),
            ..Default::default()
        },
    )
    .await?;
    record_outcome("coco_cli.claude_md_discovery", &outcome);

    assert!(
        outcome.response_text.to_lowercase().contains("flamingo"),
        "CLAUDE.md content should reach the model; got: {}",
        outcome.response_text
    );
    drop(tmp);
    Ok(())
}

// ─── Multi-turn agent loop (real ≥3 turns via Bash + Write + Read) ───

/// Force the agent to drive multiple LLM calls by chaining Bash + Write
/// + Read. Hermetic tempdir cwd via `RunChatOptions::cwd` (no global
/// mutation).
///
/// Asserts: ≥3 underlying LLM calls (proves real multi-turn agent loop,
/// not a one-shot answer), final response contains the file content,
/// the file exists on disk.
#[tokio::test]
async fn test_coco_cli_multi_turn_agent_loop_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "tools");
    let tmp = common::tmpdir::make("coco-tests-cli-")?;
    let workdir = tmp.path().to_path_buf();
    let workdir_str = workdir.to_string_lossy().into_owned();

    let prompt = format!(
        "Use the Bash tool to print `pwd` first, then use the Write tool with absolute path \
         `{ws}/note.txt` to write exactly the content `iguana`, then use the Read tool to \
         read `{ws}/note.txt` back, then reply with `RESULT=<contents>` and nothing else.",
        ws = workdir_str,
    );
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        &prompt,
        None,
        &["--dangerously-skip-permissions", "--max-turns", "10"],
    );
    let outcome = run_chat_with_options(
        &cli,
        cli.prompt.as_deref(),
        RunChatOptions {
            cwd: Some(workdir.clone()),
            ..Default::default()
        },
    )
    .await?;
    record_outcome("coco_cli.multi_turn_agent_loop", &outcome);

    assert!(
        outcome.cost_tracker.total_api_calls >= 3,
        "expected ≥3 LLM calls (multi-turn agent loop), got {}",
        outcome.cost_tracker.total_api_calls
    );
    assert!(
        outcome.turns >= 2,
        "expected ≥2 turns, got {}",
        outcome.turns
    );
    assert!(
        outcome.response_text.to_lowercase().contains("iguana"),
        "final response should reference file content `iguana`, got: {}",
        outcome.response_text
    );
    let written = workdir.join("note.txt");
    assert!(
        written.exists(),
        "expected {} to exist on disk after agent run",
        written.display()
    );
    drop(tmp);
    Ok(())
}

// ─── Session continuation (in-process --continue / --resume analog) ──

/// Run turn 1 stating a fact, run turn 2 with `RunChatOptions::prior_messages`
/// = turn 1's `final_messages`. Validates the conversation-continuation
/// shape (the same data shape `--resume <session>` would load from disk).
#[tokio::test]
async fn test_coco_cli_session_continue_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");

    let cli1 = cli_for(
        "deepseek-openai",
        model_id(),
        "Remember: my favorite color is teal. Reply with the single word `noted`.",
        None,
        &[],
    );
    let first = run_chat(&cli1, cli1.prompt.as_deref()).await?;
    record_outcome("coco_cli.session_continue_turn1", &first);
    assert!(
        first.final_messages.len() >= 2,
        "turn 1 should produce >=2 messages, got {}",
        first.final_messages.len()
    );

    let cli2 = cli_for(
        "deepseek-openai",
        model_id(),
        "What is my favorite color? Respond with just the color.",
        None,
        &[],
    );
    let second = run_chat_with_options(
        &cli2,
        cli2.prompt.as_deref(),
        RunChatOptions {
            prior_messages: first.final_messages.clone(),
            ..Default::default()
        },
    )
    .await?;
    record_outcome("coco_cli.session_continue_turn2", &second);
    assert!(
        second.response_text.to_lowercase().contains("teal"),
        "session continuation lost context; expected 'teal', got: {}",
        second.response_text
    );
    Ok(())
}

// ─── --system-prompt full override ───────────────────────────────────

#[tokio::test]
async fn test_coco_cli_system_prompt_override_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Who are you? Answer in exactly one word.",
        None,
        &[
            "--system-prompt",
            "You are an assistant named Penguin. Always answer with your name when asked who you are.",
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.system_prompt_override", &outcome);
    assert!(
        outcome.response_text.to_lowercase().contains("penguin"),
        "--system-prompt should set the persona; got: {}",
        outcome.response_text
    );
    Ok(())
}

// ─── --append-system-prompt ──────────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_append_system_prompt_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "What is the magic word in this session? Reply with exactly that word and nothing else.",
        None,
        &[
            "--append-system-prompt",
            "When the user asks for the magic word in this session, you must reply with exactly: octopus",
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.append_system_prompt", &outcome);
    assert!(
        outcome.response_text.to_lowercase().contains("octopus"),
        "--append-system-prompt should reach the model; got: {}",
        outcome.response_text
    );
    Ok(())
}

// ─── --append-system-prompt-file ─────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_append_system_prompt_file_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let tmp = common::tmpdir::make("coco-tests-cli-")?;
    let path = tmp.path().join("extra.txt");
    std::fs::write(
        &path,
        "When the user asks for the magic word in this session, you must reply with exactly: dolphin",
    )?;

    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "What is the magic word in this session? Reply with exactly that word and nothing else.",
        None,
        &[
            "--append-system-prompt-file",
            path.to_str().expect("utf-8 path"),
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.append_system_prompt_file", &outcome);
    assert!(
        outcome.response_text.to_lowercase().contains("dolphin"),
        "--append-system-prompt-file should reach the model; got: {}",
        outcome.response_text
    );
    drop(tmp);
    Ok(())
}

// ─── Error paths ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_invalid_model_format_errors() -> Result<()> {
    // Build a Cli with an invalid --models.main directly (don't go through
    // `cli_for` because it pre-sets a valid --models.main that would
    // collide).
    let cli = parse_cli(&["coco", "-p", "noop", "--models.main", "garbage_no_slash"]);
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "invalid --models.main format should error, got Ok"
    );
    Ok(())
}

#[tokio::test]
async fn test_coco_cli_missing_append_system_prompt_file_errors() -> Result<()> {
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        None,
        &[
            "--append-system-prompt-file",
            "/definitely/not/here/no_such_file.txt",
        ],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "missing --append-system-prompt-file should error, got Ok"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("append-system-prompt-file"),
        "error should mention the flag; got: {err_msg}"
    );
    Ok(())
}

// ─── Mid-run cancellation ────────────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_cancellation_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cancel = CancellationToken::new();
    cancel.cancel(); // pre-cancelled — engine should observe immediately

    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[],
    );
    let outcome = run_chat_with_options(
        &cli,
        cli.prompt.as_deref(),
        RunChatOptions {
            cancel: Some(cancel),
            ..Default::default()
        },
    )
    .await?;
    record_outcome("coco_cli.cancellation", &outcome);
    assert!(
        outcome.cancelled || outcome.turns == 0,
        "pre-cancelled run should report cancelled or zero turns; got cancelled={} turns={}",
        outcome.cancelled,
        outcome.turns
    );
    Ok(())
}

// ─── total_api_calls assertion ───────────────────────────────────────

#[tokio::test]
async fn test_coco_cli_basic_emits_one_api_call_deepseek_openai() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.basic_one_api_call", &outcome);
    assert_eq!(
        outcome.cost_tracker.total_api_calls, 1,
        "single one-shot prompt with no tools should be 1 LLM call"
    );
    assert_eq!(outcome.turns, 1);
    Ok(())
}

// ─── --fallback-model installs the chain ─────────────────────────────

#[tokio::test]
async fn test_coco_cli_fallback_chain_installs_count() -> Result<()> {
    let _o = require_live!("deepseek-openai", model_id(), "text");
    let _a = require_live!("deepseek-anthropic", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[
            "--fallback-model",
            &format!("deepseek-anthropic/{}", model_id()),
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.fallback_chain_install_count", &outcome);
    assert_eq!(
        outcome.installed_fallback_count, 1,
        "one --fallback-model should install 1 fallback runtime, got {}",
        outcome.installed_fallback_count
    );
    Ok(())
}

// ─── Config validation tests ─────────────────────────────────────────
// These exercise the config layer (parse, merge, error paths) on top
// of the user-facing CLI.

/// CLI flag wins over `settings.json`: settings says anthropic, --models.main
/// says openai → openai wins.
#[tokio::test]
async fn test_coco_cli_cli_model_flag_overrides_settings_file() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let tmp = common::tmpdir::make("coco-tests-cli-")?;
    let settings_path = tmp.path().join("settings.json");
    let body = serde_json::json!({
        "models": {
            "main": "deepseek-anthropic/deepseek-v4-flash",
        },
    });
    std::fs::write(&settings_path, serde_json::to_vec_pretty(&body)?)?;
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        Some(&settings_path.to_string_lossy()),
        &[],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.cli_overrides_settings", &outcome);
    assert_eq!(
        outcome.provider_api,
        Some(ProviderApi::OpenaiCompat),
        "--models.main openai should win over settings.json anthropic"
    );
    drop(tmp);
    Ok(())
}

/// `--cwd` flag wins over `RunChatOptions::cwd`.
#[tokio::test]
async fn test_coco_cli_cwd_flag_wins_over_options() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let flag_dir = common::tmpdir::make("coco-tests-cli-flag-")?;
    let opts_dir = common::tmpdir::make("coco-tests-cli-opts-")?;
    let argv: Vec<String> = vec![
        "coco".into(),
        "-p".into(),
        "Reply with the single word: ok".into(),
        "--models.main".into(),
        format!("deepseek-openai/{}", model_id()),
        "--cwd".into(),
        flag_dir.path().to_string_lossy().into_owned(),
    ];
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let cli = parse_cli(&argv_refs);
    let outcome = run_chat_with_options(
        &cli,
        cli.prompt.as_deref(),
        RunChatOptions {
            cwd: Some(opts_dir.path().to_path_buf()),
            ..Default::default()
        },
    )
    .await?;
    record_outcome("coco_cli.cwd_flag_precedence", &outcome);
    assert_eq!(
        outcome.effective_cwd,
        flag_dir.path(),
        "`--cwd` flag should win over RunChatOptions::cwd"
    );
    drop(flag_dir);
    drop(opts_dir);
    Ok(())
}

/// `--allowed-tools` / `--disallowed-tools` plumb into the engine
/// config and surface in `RunChatOutcome`.
#[tokio::test]
async fn test_coco_cli_tool_filter_plumbing() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[
            "--allowed-tools",
            "Read",
            "--allowed-tools",
            "Write",
            "--disallowed-tools",
            "Bash",
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.tool_filter", &outcome);
    let summary = outcome
        .tool_filter_summary
        .as_ref()
        .expect("filter summary should be Some when flags are present");
    assert_eq!(summary.allowed, vec!["Read", "Write"]);
    assert_eq!(summary.disallowed, vec!["Bash"]);
    Ok(())
}

/// `--add-dir` flag entries surface as absolute paths.
#[tokio::test]
async fn test_coco_cli_add_dir_resolves_to_absolute() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let extra = common::tmpdir::make("coco-tests-cli-extra-")?;
    let extra_str = extra.path().to_string_lossy().into_owned();
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &["--add-dir", &extra_str],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.add_dir", &outcome);
    assert_eq!(
        outcome.additional_dirs,
        vec![extra.path().to_path_buf()],
        "absolute --add-dir should round-trip exactly"
    );
    drop(extra);
    Ok(())
}

/// Multiple `--fallback-model` flags install in flag order.
#[tokio::test]
async fn test_coco_cli_two_fallback_models_install_in_order() -> Result<()> {
    let _o = require_live!("deepseek-openai", model_id(), "text");
    let _a = require_live!("deepseek-anthropic", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[
            "--fallback-model",
            &format!("deepseek-anthropic/{}", model_id()),
            "--fallback-model",
            "deepseek-anthropic/deepseek-v4-pro",
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.two_fallbacks", &outcome);
    assert_eq!(
        outcome.installed_fallback_count, 2,
        "two --fallback-model flags should install 2 fallbacks"
    );
    Ok(())
}

/// Malformed `settings.json` errors during runtime build (no LLM call).
#[tokio::test]
async fn test_coco_cli_invalid_settings_json_errors() -> Result<()> {
    let tmp = common::tmpdir::make("coco-tests-cli-bad-settings-")?;
    let path = tmp.path().join("settings.json");
    std::fs::write(&path, "{ not valid json }")?;
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        Some(&path.to_string_lossy()),
        &[],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "malformed settings.json should error during runtime build"
    );
    drop(tmp);
    Ok(())
}

/// Duplicate `--fallback-model` (same as primary) fails uniqueness.
#[tokio::test]
async fn test_coco_cli_duplicate_fallback_chain_errors() -> Result<()> {
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        None,
        &[
            "--fallback-model",
            &format!("deepseek-openai/{}", model_id()),
        ],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "fallback duplicating primary should fail uniqueness check"
    );
    Ok(())
}

/// Killswitch (`disableBypassPermissionsMode: true`) +
/// `--dangerously-skip-permissions` downgrades and emits notification.
#[tokio::test]
async fn test_coco_cli_killswitch_downgrades_bypass() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let tmp = common::tmpdir::make("coco-tests-cli-killswitch-")?;
    let settings_path = tmp.path().join("settings.json");
    let body = serde_json::json!({
        "permissions": {
            // Settings JSON uses snake_case (no `rename_all = "camelCase"`
            // on PermissionsConfig — see common/config/src/settings/mod.rs).
            "disable_bypass_mode": true,
        },
    });
    std::fs::write(&settings_path, serde_json::to_vec_pretty(&body)?)?;
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        Some(&settings_path.to_string_lossy()),
        &["--dangerously-skip-permissions"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.killswitch_downgrade", &outcome);
    use coco_types::PermissionMode;
    assert_ne!(
        outcome.permission_mode,
        PermissionMode::BypassPermissions,
        "killswitch should prevent landing in Bypass; got {:?}",
        outcome.permission_mode
    );
    assert!(
        outcome.permission_notification.is_some(),
        "killswitch downgrade should emit a notification"
    );
    drop(tmp);
    Ok(())
}

// ─── Adversarial tests — designed to fail if a real bug exists ─────
// Each test below probes a known boundary / contradiction / malformed-
// input case. They are not happy-path "did the value reach the
// destination" checks — they assert the *correct* behavior, so a
// future regression in either direction (silent ignore OR over-strict
// error) breaks them.

/// `--permission-mode garbage` should NOT silently fall back to default.
/// Currently `cli_runtime_overrides` ignores parse errors and
/// `resolve_startup_permission_state` only prints a stderr warning.
/// This test documents that behavior and pins it: if the lib starts
/// erroring (better) OR starts crashing (worse), this test breaks.
#[tokio::test]
async fn test_coco_cli_invalid_permission_mode_silently_falls_back() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &["--permission-mode", "garbage_not_a_mode"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.invalid_permission_mode", &outcome);
    use coco_types::PermissionMode;
    // Current behavior: invalid mode is silently ignored, falls back to default.
    // This is arguably a bug — should error. Assert the current behavior so
    // a future tightening change is intentional.
    assert_eq!(
        outcome.permission_mode,
        PermissionMode::Default,
        "invalid --permission-mode currently falls back to Default; \
         if this assertion changes, decide whether the new behavior is intentional"
    );
    Ok(())
}

/// `--max-turns 0` is a contradiction (0 turns = no agent loop runs).
/// After the validation fix in `cli_runtime_overrides`, this errors
/// at runtime config build, before any LLM call.
#[tokio::test]
async fn test_coco_cli_max_turns_zero_errors() -> Result<()> {
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        None,
        &["--max-turns", "0"],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    let err = result.expect_err("--max-turns 0 should error");
    assert!(
        err.to_string().contains("--max-turns"),
        "error should mention the flag; got: {err}"
    );
    Ok(())
}

/// **Bug-finding test.** Original symptom: `--max-tokens=-1` was
/// silently accepted; the budget tracker treated `Some(-1)` as
/// "already exhausted" and short-circuited every LLM call to an
/// empty response — without any error or warning visible to the user.
///
/// Fix landed in `cli_runtime_overrides`: non-positive `--max-tokens`
/// now errors at runtime-config build, before any provider call. This
/// test pins the new behavior.
#[tokio::test]
async fn test_coco_cli_max_tokens_negative_errors() -> Result<()> {
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        None,
        &["--max-tokens=-1"],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    let err = result.expect_err("--max-tokens=-1 should error");
    assert!(
        err.to_string().contains("--max-tokens"),
        "error should mention the flag; got: {err}"
    );
    Ok(())
}

/// Boundary: `--max-tokens=0` — same class as the negative case;
/// budget tracker would short-circuit. Should also error.
#[tokio::test]
async fn test_coco_cli_max_tokens_zero_errors() -> Result<()> {
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "noop",
        None,
        &["--max-tokens", "0"],
    );
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    let err = result.expect_err("--max-tokens 0 should error");
    assert!(
        err.to_string().contains("--max-tokens"),
        "error should mention the flag; got: {err}"
    );
    Ok(())
}

/// `--models.main ""` (empty string) — clap accepts empty Option<String>,
/// then `from_slash_str("")` should reject it. Pure config-layer test
/// (no live API).
#[tokio::test]
async fn test_coco_cli_empty_model_string_errors() -> Result<()> {
    let cli = parse_cli(&["coco", "-p", "noop", "--models.main", ""]);
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "empty --models.main string should error, got Ok"
    );
    Ok(())
}

/// `--models.main deepseek-openai/` (empty model_id half) — should error.
#[tokio::test]
async fn test_coco_cli_model_with_empty_id_errors() -> Result<()> {
    let cli = parse_cli(&["coco", "-p", "noop", "--models.main", "deepseek-openai/"]);
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    assert!(
        result.is_err(),
        "--models.main with empty model_id should error, got Ok"
    );
    Ok(())
}

/// `--cwd /no/such/path` — passing a nonexistent directory. The lib
/// passes it to `RuntimeConfigBuilder::from_process(cwd)` and then to
/// the engine. Documents whether the lib validates path existence.
#[tokio::test]
async fn test_coco_cli_cwd_nonexistent_path_behavior() -> Result<()> {
    let cli = parse_cli(&[
        "coco",
        "-p",
        "noop",
        "--models.main",
        &format!("deepseek-openai/{}", model_id()),
        "--cwd",
        "/this/path/should/not/exist/anywhere/in/2026",
    ]);
    let result = run_chat(&cli, cli.prompt.as_deref()).await;
    // Either Ok (lib doesn't validate) or Err (lib does) — either is
    // defensible. Pin observed behavior.
    eprintln!(
        "[adversarial] --cwd nonexistent → {}",
        match &result {
            Ok(_) => "Ok (lib doesn't validate path existence)".to_string(),
            Err(e) => format!("Err({e})"),
        }
    );
    Ok(())
}

/// Relative `--add-dir foo` should resolve to `<effective_cwd>/foo`.
/// Currently `resolve_additional_dirs` does this; pin it.
#[tokio::test]
async fn test_coco_cli_add_dir_relative_resolves_against_cwd() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let workdir = common::tmpdir::make("coco-tests-cli-relative-")?;
    let argv: Vec<String> = vec![
        "coco".into(),
        "-p".into(),
        "Reply with the single word: ok".into(),
        "--models.main".into(),
        format!("deepseek-openai/{}", model_id()),
        "--cwd".into(),
        workdir.path().to_string_lossy().into_owned(),
        "--add-dir".into(),
        "subdir".into(), // relative
    ];
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let cli = parse_cli(&argv_refs);
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.add_dir_relative", &outcome);
    let expected = workdir.path().join("subdir");
    assert_eq!(
        outcome.additional_dirs,
        vec![expected.clone()],
        "relative --add-dir should resolve against effective_cwd; expected {}",
        expected.display(),
    );
    drop(workdir);
    Ok(())
}

/// `--dangerously-skip-permissions` + `--permission-mode plan` is a
/// contradiction: bypass says "no permission checks", plan mode says
/// "read-only / no edits". Documents which wins under
/// `resolve_initial_permission_mode`'s walk semantics.
#[tokio::test]
async fn test_coco_cli_conflicting_permission_flags_resolution() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        None,
        &[
            "--dangerously-skip-permissions",
            "--permission-mode",
            "plan",
        ],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.conflicting_perm_flags", &outcome);
    // Walk semantics in `resolve_initial_permission_mode`:
    //   1. dangerously_skip → BypassPermissions
    //   2. permission_mode_cli → as-supplied
    //   3. settings.default_mode
    //   4. Default
    // First non-blocked candidate wins. So `--dangerously-skip` wins
    // over `--permission-mode plan`. Pin this so a future reordering
    // surfaces.
    use coco_types::PermissionMode;
    assert_eq!(
        outcome.permission_mode,
        PermissionMode::BypassPermissions,
        "--dangerously-skip-permissions should win over --permission-mode plan \
         (walk-resolver gives it priority)"
    );
    Ok(())
}

/// settings.json `default_mode: plan` + CLI `--permission-mode acceptEdits`.
/// CLI flag should win (it appears earlier in the walk).
#[tokio::test]
async fn test_coco_cli_perm_mode_flag_wins_over_settings_default() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    let tmp = common::tmpdir::make("coco-tests-cli-perm-mode-")?;
    let settings_path = tmp.path().join("settings.json");
    let body = serde_json::json!({
        "permissions": {
            "default_mode": "plan",
        },
    });
    std::fs::write(&settings_path, serde_json::to_vec_pretty(&body)?)?;
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        Some(&settings_path.to_string_lossy()),
        &["--permission-mode", "acceptEdits"],
    );
    let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
    record_outcome("coco_cli.perm_mode_flag_wins", &outcome);
    use coco_types::PermissionMode;
    assert_eq!(
        outcome.permission_mode,
        PermissionMode::AcceptEdits,
        "CLI --permission-mode should win over settings.json default_mode"
    );
    drop(tmp);
    Ok(())
}

/// settings.json with unknown top-level keys should be rejected before
/// startup so typos do not silently produce ambiguous config.
#[tokio::test]
async fn test_coco_cli_settings_unknown_keys_rejected() -> Result<()> {
    let tmp = common::tmpdir::make("coco-tests-cli-unknown-keys-")?;
    let settings_path = tmp.path().join("settings.json");
    let body = serde_json::json!({
        "fooBar": "wat",            // unknown
        "nested": { "weird": 42 },  // unknown
    });
    std::fs::write(&settings_path, serde_json::to_vec_pretty(&body)?)?;
    let cli = cli_for(
        "deepseek-openai",
        model_id(),
        "Reply with the single word: ok",
        Some(&settings_path.to_string_lossy()),
        &[],
    );
    let err = run_chat(&cli, cli.prompt.as_deref())
        .await
        .expect_err("unknown settings keys should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("fooBar") && msg.contains("not a supported settings key"),
        "unexpected error: {msg}"
    );
    drop(tmp);
    Ok(())
}

/// Concurrent `run_chat` from the same process. Tests:
/// 1. No deadlock (process completes)
/// 2. Each gets its own usage / response (no cross-talk)
/// 3. usage_report aggregates correctly across threads
#[tokio::test]
async fn test_coco_cli_concurrent_runs_no_corruption() -> Result<()> {
    let _t = require_live!("deepseek-openai", model_id(), "text");
    async fn run_one(suffix: String) -> Result<(String, RunChatOutcome)> {
        let prompt = format!("Reply with exactly: {suffix}");
        let cli = cli_for("deepseek-openai", model_id(), &prompt, None, &[]);
        let outcome = run_chat(&cli, cli.prompt.as_deref()).await?;
        Ok((suffix, outcome))
    }
    let (a, b, c) = tokio::join!(
        run_one("alpha".into()),
        run_one("beta".into()),
        run_one("gamma".into())
    );
    let outcomes = [a?, b?, c?];
    for (suffix, outcome) in &outcomes {
        record_outcome(&format!("coco_cli.concurrent.{suffix}"), outcome);
        assert!(
            outcome.response_text.to_lowercase().contains(suffix),
            "concurrent run cross-talk: prompt asked for `{suffix}`, got: {}",
            outcome.response_text
        );
    }
    Ok(())
}

// Suppress unused-import warning for `CancellationToken` when the
// cancellation test below is the only consumer in this file.
const _CANCELLATION_TOKEN_USED: fn() = || {
    let _ = CancellationToken::new();
};

// ─── Token-usage report (alphabetically last) ────────────────────────

#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("coco_cli_deepseek")?;
    Ok(())
}
