use crate::tools::bash::BashTool;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

// ── R7-T25: tool prompt content checks ──
//
// Verify that the BashTool model-facing prompt() includes the critical
// instructional content (avoid-native-commands list, parallel calls
// guidance, git safety protocol, sandbox-related notes). Regression
// guard against the prompt being silently truncated to a stub. The full
// guidance lives in prompt(); description() is only the short per-call
// UI label ("Run shell command").

#[tokio::test]
async fn test_bash_prompt_includes_avoid_native_commands_list() {
    let desc = <BashTool as DynTool>::prompt(&BashTool, &PromptOptions::default()).await;
    // Every native command the model is told to avoid.
    for forbidden in &["find", "grep", "cat", "head", "tail", "sed", "awk", "echo"] {
        assert!(
            desc.contains(&format!("`{forbidden}`")),
            "Bash prompt should warn about `{forbidden}`, got:\n{desc}"
        );
    }
}

#[tokio::test]
async fn test_bash_prompt_includes_tool_preferences() {
    let desc = <BashTool as DynTool>::prompt(&BashTool, &PromptOptions::default()).await;
    for tool in &["Glob", "Grep", "Read", "Edit", "Write"] {
        assert!(
            desc.contains(tool),
            "Bash prompt should mention {tool} as a preferred tool"
        );
    }
}

#[tokio::test]
async fn test_bash_prompt_includes_git_safety_protocol() {
    let desc = <BashTool as DynTool>::prompt(&BashTool, &PromptOptions::default()).await;
    assert!(desc.contains("Git Safety Protocol"));
    assert!(desc.contains("force push to main/master"));
    assert!(desc.contains("destructive git commands"));
    assert!(desc.contains("hooks (--no-verify"));
}

#[tokio::test]
async fn test_bash_prompt_includes_pr_creation_guidance() {
    let desc = <BashTool as DynTool>::prompt(&BashTool, &PromptOptions::default()).await;
    assert!(desc.contains("Creating pull requests"));
    assert!(desc.contains("gh pr create"));
}

#[test]
fn test_bash_description_is_short_label() {
    // `command` is required (no `#[serde(default)]`), so the no-description
    // fallback is exercised with a command present but `description` omitted.
    let desc = <BashTool as DynTool>::description(
        &BashTool,
        &json!({ "command": "ls" }),
        &DescriptionOptions::default(),
    );
    assert_eq!(desc, "Run shell command");
    let desc_with_input = <BashTool as DynTool>::description(
        &BashTool,
        &json!({ "command": "ls", "description": "List files" }),
        &DescriptionOptions::default(),
    );
    assert_eq!(desc_with_input, "List files");
}

// ---------------------------------------------------------------------------
// Multi-stage permission pipeline
// ---------------------------------------------------------------------------

/// Read-only fast path: `cat`, `ls`, `grep`, `git log`, etc. must be reported
/// as non-destructive and concurrency-safe so the executor auto-approves and
/// batches them with other safe tools.
#[test]
fn test_bash_read_only_fast_path() {
    let cases = [
        "cat README.md",
        "ls -la",
        "grep foo file.txt",
        "git log --oneline",
        "pwd",
        "echo hello",
        "head -n 5 file",
        "tail -f log",
        "wc -l file",
        "which cargo",
    ];
    for cmd in cases {
        let input = json!({"command": cmd});
        assert!(
            <BashTool as DynTool>::is_read_only(&BashTool, &input),
            "`{cmd}` should be read-only"
        );
        assert!(
            <BashTool as DynTool>::is_concurrency_safe(&BashTool, &input),
            "`{cmd}` should be concurrency-safe"
        );
        assert!(
            !<BashTool as DynTool>::is_destructive(&BashTool, &input),
            "`{cmd}` should not be destructive"
        );
    }
}

#[test]
fn test_bash_cd_safety_parity() {
    let cwd = std::env::current_dir().expect("test cwd");
    let noop_cd_git = json!({"command": format!("cd {} && git diff", cwd.display())});
    assert!(<BashTool as DynTool>::is_read_only(&BashTool, &noop_cd_git));
    assert!(<BashTool as DynTool>::is_concurrency_safe(
        &BashTool,
        &noop_cd_git
    ));

    let readonly = json!({"command": "cd src && ls"});
    assert!(<BashTool as DynTool>::is_read_only(&BashTool, &readonly));
    assert!(<BashTool as DynTool>::is_concurrency_safe(
        &BashTool, &readonly
    ));

    let cd_git = json!({"command": "cd src && git status"});
    assert!(!<BashTool as DynTool>::is_read_only(&BashTool, &cd_git));
    assert!(!<BashTool as DynTool>::is_concurrency_safe(
        &BashTool, &cd_git
    ));

    let multiple_cd = json!({"command": "cd a && cd b && ls"});
    assert!(!<BashTool as DynTool>::is_read_only(
        &BashTool,
        &multiple_cd
    ));
    assert!(!<BashTool as DynTool>::is_concurrency_safe(
        &BashTool,
        &multiple_cd
    ));
}

/// Non-read-only commands (mutations, installs, shell execution) must be
/// reported as destructive so the permission evaluator asks the user.
///
/// NOTE: output redirect detection (`echo x > file`) is not yet handled
/// by `coco_shell::read_only::is_read_only_command` — that's a known
/// upstream gap in the allowlist approach and tracked separately. For now
/// we only test commands where the first word itself is destructive.
#[test]
fn test_bash_destructive_commands() {
    let cases = [
        "rm -rf /tmp/data",
        "mv a b",
        "cp a b",
        "npm install",
        "git commit -m 'x'",
        "touch file",
        "mkdir -p foo",
    ];
    for cmd in cases {
        let input = json!({"command": cmd});
        assert!(
            !<BashTool as DynTool>::is_read_only(&BashTool, &input),
            "`{cmd}` should not be read-only"
        );
        assert!(
            <BashTool as DynTool>::is_destructive(&BashTool, &input),
            "`{cmd}` should be destructive"
        );
    }
}

/// Missing command → `command` is now required, so `{}` fails to deserialize
/// into `BashInput`. The `DynTool` blanket impls fall back to their
/// deserialize-failure defaults: not read-only, not concurrency-safe, and
/// (since `is_destructive` cannot prove the command is read-only) not flagged
/// destructive either. The net effect is still conservative — such input never
/// gets auto-batched as a safe read-only call; it's rejected at validation
/// before any execution decision matters.
#[test]
fn test_bash_missing_command_conservative() {
    let input = json!({});
    assert!(!<BashTool as DynTool>::is_read_only(&BashTool, &input));
    assert!(!<BashTool as DynTool>::is_concurrency_safe(
        &BashTool, &input
    ));
    assert!(!<BashTool as DynTool>::is_destructive(&BashTool, &input));
}

/// shell-163: `eval` and `IFS=` injection are routed through the
/// *ask* permission flow, NOT hard-failed at the Deny gate. The security
/// check returns `behavior: 'ask'` (never `'deny'`) for these — the
/// user can approve them through the normal permission prompt. BashTool only
/// hard-fails on `SecuritySeverity::Deny` (bash.rs), which is now reserved for
/// genuinely-catastrophic constructs (raw control chars, `/proc/*/environ`).
#[tokio::test]
async fn test_bash_security_eval_routes_through_ask_not_deny() {
    let checks = coco_shell::security::check_security("eval $user_input");
    assert!(
        checks
            .iter()
            .any(|c| c.severity == coco_shell::security::SecuritySeverity::Ask),
        "eval must surface an Ask-severity check"
    );
    assert!(
        !checks
            .iter()
            .any(|c| c.severity == coco_shell::security::SecuritySeverity::Deny),
        "eval must NOT be hard-Deny — TS routes it through ask"
    );
}

#[tokio::test]
async fn test_bash_security_ifs_injection_routes_through_ask_not_deny() {
    let checks = coco_shell::security::check_security("IFS=: read -r a b");
    assert!(
        checks
            .iter()
            .any(|c| c.severity == coco_shell::security::SecuritySeverity::Ask),
        "IFS injection must surface an Ask-severity check"
    );
    assert!(
        !checks
            .iter()
            .any(|c| c.severity == coco_shell::security::SecuritySeverity::Deny),
        "IFS must NOT be hard-Deny — TS routes it through ask"
    );
}

/// Read-only commands MUST skip the security Deny check so benign patterns
/// like `grep 'foo`bar' file` (which may contain metacharacters inside quoted
/// strings) don't trigger false positives.
#[tokio::test]
async fn test_bash_read_only_skips_security_checks() {
    let ctx = ToolUseContext::test_default();
    // This grep command has a backtick inside the pattern; security check
    // would Ask on it, but read-only fast path should skip that check.
    let result =
        <BashTool as DynTool>::execute(&BashTool, json!({"command": "echo hello"}), &ctx).await;
    assert!(result.is_ok(), "echo should run without security check");
}

// ---------------------------------------------------------------------------
// B4.3: resolved Bash timeout config
// ---------------------------------------------------------------------------

#[test]
fn test_bash_default_timeout_from_default_config() {
    let config = coco_config::ToolConfig::default();
    assert_eq!(super::default_timeout_ms(&config), 120_000);
}

#[test]
fn test_bash_default_timeout_from_runtime_config() {
    let mut config = coco_config::ToolConfig::default();
    config.bash.default_timeout_ms = 30_000;
    assert_eq!(super::default_timeout_ms(&config), 30_000);
}

#[test]
fn test_bash_default_timeout_zero_clamps_to_one() {
    let mut config = coco_config::ToolConfig::default();
    config.bash.default_timeout_ms = 0;
    assert_eq!(super::default_timeout_ms(&config), 1);
}

// Does not enforce a max timeout (the configured max is only an
// advisory schema hint), so there is no `max_timeout_ms` helper to test.
#[test]
fn test_bash_timeout_above_config_max_is_not_rejected() {
    let ctx = ToolUseContext::test_default();
    let vr = <BashTool as DynTool>::validate_input(
        &BashTool,
        &json!({"command": "echo hi", "timeout": 9_999_999u64}),
        &ctx,
    );
    assert!(matches!(vr, coco_tool_runtime::ValidationResult::Valid));
}

// ---------------------------------------------------------------------------
// B4.2: timeout behaviour
// ---------------------------------------------------------------------------
//
// The "auto-background-on-timeout suggestion" path that lived here is
// gone — the W3 unified TaskRuntime path (`execute_via_task_runtime`)
// catches timeouts inside `task_runtime::run_shell_to_completion` via
// `WaitOutcome::TimedOut`, flips `interrupted: true`, and returns
// `Ok(ToolResult)` instead of `Err`. Auto-detach (`auto_detach_ms`)
// supersedes the old "hint the model to retry" mechanism. The
// remaining test below covers the legacy no-TaskHandle fallback in
// `execute_foreground`, which is the only surviving Err-with-timeout-wording
// code path.

/// Without a TaskHandle, the tool should NOT suggest background
/// retry (it's not available) — just a plain timeout error.
#[tokio::test]
async fn test_bash_timeout_error_no_suggestion_without_handle() {
    let ctx = ToolUseContext::test_default();
    // test_default has task_handle = None.
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "sleep 10", "timeout": 100}),
        &ctx,
    )
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("timed out"));
    assert!(
        !err.contains("run_in_background"),
        "should not suggest bg retry when unavailable: {err}"
    );
}

#[tokio::test]
async fn test_bash_echo() {
    let ctx = ToolUseContext::test_default();
    let result =
        <BashTool as DynTool>::execute(&BashTool, json!({"command": "echo hello world"}), &ctx)
            .await
            .unwrap();

    // R5-T14: structured output — read stdout directly.
    assert!(
        result.data["stdout"]
            .as_str()
            .unwrap()
            .contains("hello world")
    );
    assert_eq!(result.data["exitCode"], 0);
    assert_eq!(result.data["interrupted"], false);
}

#[tokio::test]
async fn test_bash_exit_code() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(&BashTool, json!({"command": "exit 42"}), &ctx)
        .await
        .unwrap();

    // R5-T14: exitCode is a dedicated field now.
    assert_eq!(result.data["exitCode"], 42);
}

#[tokio::test]
async fn test_bash_stderr() {
    let ctx = ToolUseContext::test_default();
    let result =
        <BashTool as DynTool>::execute(&BashTool, json!({"command": "echo err >&2"}), &ctx)
            .await
            .unwrap();

    // R5-T14: stderr has its own field.
    assert!(result.data["stderr"].as_str().unwrap().contains("err"));
    assert_eq!(result.data["stdout"].as_str().unwrap(), "");
}

#[tokio::test]
async fn test_bash_timeout() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "sleep 10", "timeout": 100}),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("timed out"));
}

#[tokio::test]
async fn test_bash_pwd() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(&BashTool, json!({"command": "pwd"}), &ctx)
        .await
        .unwrap();

    assert!(!result.data["stdout"].as_str().unwrap().is_empty());
}

/// `BASH_MAX_OUTPUT_DEFAULT = 30_000`. Our Bash tool must advertise the same
/// persistence threshold so cross-runtime sessions handle large outputs
/// identically. Regression guard for R4-T6.
#[test]
fn test_bash_max_result_size_bound_matches_ts() {
    assert_eq!(
        <BashTool as DynTool>::max_result_size_bound(&BashTool,),
        coco_tool_runtime::ResultSizeBound::Chars(30_000),
    );
}

/// The bash-side helper is a plain cast; upper clamping lives in
/// `coco_config::BashConfig::finalize()` (verified in sections.test.rs).
/// This test just pins the pass-through semantics.
#[test]
fn test_bash_max_output_bytes_pass_through() {
    let mut config = coco_config::ToolConfig::default();
    assert_eq!(super::max_output_bytes(&config), 30_000);

    config.bash.max_output_bytes = 50_000;
    assert_eq!(super::max_output_bytes(&config), 50_000);

    // Negative values are normalized to 0 by the cast guard.
    config.bash.max_output_bytes = -1;
    assert_eq!(super::max_output_bytes(&config), 0);
}

/// The shell's cwd is overridden for isolated subagents via `ctx.cwd_override`
/// — the foreground shell must run inside that directory so worktree-isolated
/// tasks don't leak into the host process cwd. Regression guard for R4-T5.
#[tokio::test]
async fn test_bash_respects_cwd_override() {
    let dir = tempfile::tempdir().unwrap();
    let canon = std::fs::canonicalize(dir.path()).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(canon.clone());

    let result = <BashTool as DynTool>::execute(&BashTool, json!({"command": "pwd"}), &ctx)
        .await
        .unwrap();

    let stdout = result.data["stdout"].as_str().unwrap();
    // `pwd` output has a trailing newline — compare on the trimmed prefix.
    assert!(
        stdout.trim_end().contains(canon.to_str().unwrap()),
        "pwd should report the cwd_override directory; got: {stdout:?}, expected: {canon:?}"
    );
}

#[tokio::test]
async fn test_bash_piped_command() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "echo -e 'a\\nb\\nc' | wc -l"}),
        &ctx,
    )
    .await
    .unwrap();

    assert!(result.data["stdout"].as_str().unwrap().contains('3'));
}

/// R5-T14: `true` exits 0 with empty output. In the new structured
/// shape, stdout/stderr are empty strings and exitCode is 0 — there is
/// no "(no output)" sentinel anymore.
#[tokio::test]
async fn test_bash_no_output() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(&BashTool, json!({"command": "true"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result.data["stdout"].as_str().unwrap(), "");
    assert_eq!(result.data["stderr"].as_str().unwrap(), "");
    assert_eq!(result.data["exitCode"], 0);
    assert_eq!(result.data["interrupted"], false);
}

#[tokio::test]
async fn test_bash_with_progress_channel() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = ToolUseContext::test_default();
    ctx.progress_tx = Some(tx);

    let result = <BashTool as DynTool>::execute(&BashTool, json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();

    // Should have received at least the initial "running" progress
    let mut progress_msgs = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        progress_msgs.push(msg);
    }

    assert!(
        !progress_msgs.is_empty(),
        "should receive at least one progress message"
    );
    assert_eq!(progress_msgs[0].data["type"], "bash_progress");
    assert_eq!(progress_msgs[0].data["status"], "running");

    assert!(result.data["stdout"].as_str().unwrap().contains("hello"));
}

// ---------------------------------------------------------------------------
// R6-T18: sandbox decision
// ---------------------------------------------------------------------------

use super::active_sandbox_state;

/// `Feature::Sandbox` disabled → no sandbox state surfaces, even if
/// the bootstrap layer installed one. Callsite gate for the runtime.
#[test]
fn test_active_sandbox_state_feature_disabled_returns_none() {
    let mut ctx = ToolUseContext::test_default();
    ctx.features = std::sync::Arc::new(coco_types::Features::empty());
    ctx.sandbox_state = Some(std::sync::Arc::new(coco_sandbox::SandboxState::disabled()));
    assert!(active_sandbox_state(&ctx).is_none());
}

/// `Feature::Sandbox` enabled but no state installed (test/headless path)
/// → returns None.
#[test]
fn test_active_sandbox_state_no_bootstrap_returns_none() {
    let mut ctx = ToolUseContext::test_default();
    let mut features = coco_types::Features::empty();
    features.set_enabled(coco_types::Feature::Sandbox, true);
    ctx.features = std::sync::Arc::new(features);
    ctx.sandbox_state = None;
    assert!(active_sandbox_state(&ctx).is_none());
}

/// Decision evaluation belongs on `SandboxState::command_snapshot`.
/// Verify the snapshot reads the `dangerouslyDisableSandbox` bypass
/// path when the state is `external` (so platform_active doesn't
/// matter for the test).
#[test]
fn test_sandbox_state_bypass_unsandboxes() {
    let settings = coco_sandbox::SandboxSettings {
        enabled: true,
        allow_unsandboxed_commands: true,
        ..Default::default()
    };
    let state = coco_sandbox::SandboxState::external(
        coco_sandbox::EnforcementLevel::WorkspaceWrite,
        settings,
        coco_sandbox::SandboxConfig::default(),
    );
    let snap = state.command_snapshot("rm -rf /", coco_sandbox::SandboxBypass::Requested);
    assert!(!snap.should_wrap, "bypass + allow_unsandboxed → no wrap");
}

#[test]
fn test_sandbox_state_excluded_command_unsandboxes() {
    let settings = coco_sandbox::SandboxSettings {
        enabled: true,
        excluded_commands: vec!["git".into()],
        ..Default::default()
    };
    let state = coco_sandbox::SandboxState::external(
        coco_sandbox::EnforcementLevel::WorkspaceWrite,
        settings,
        coco_sandbox::SandboxConfig::default(),
    );
    let snap = state.command_snapshot("git status", coco_sandbox::SandboxBypass::No);
    assert!(!snap.should_wrap, "excluded command → no wrap");
}

#[test]
fn test_sandbox_state_active_non_excluded_wraps() {
    let settings = coco_sandbox::SandboxSettings {
        enabled: true,
        ..Default::default()
    };
    let state = coco_sandbox::SandboxState::external(
        coco_sandbox::EnforcementLevel::WorkspaceWrite,
        settings,
        coco_sandbox::SandboxConfig::default(),
    );
    let snap = state.command_snapshot("ls -la", coco_sandbox::SandboxBypass::No);
    assert!(snap.should_wrap, "active + non-excluded → wrap");
}

/// Auto-background-on-timeout defaults ON.
#[test]
fn test_auto_background_on_timeout_default_enabled() {
    let config = coco_config::ToolConfig::default();
    assert!(config.bash.auto_background_on_timeout);
}

/// `sleep` is excluded from auto-backgrounding, so a `sleep` timeout still
/// surfaces as an ExecutionFailed error rather than moving to the background.
/// (`test_default()` also has no TaskRuntime, exercising the fallback path.)
#[tokio::test]
async fn test_bash_sleep_timeout_errors_not_backgrounded() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "sleep 10", "timeout": 100}),
        &ctx,
    )
    .await;
    assert!(
        result.is_err(),
        "sleep timeout should error, not background"
    );
    let err = result.unwrap_err().to_string();
    assert!(err.contains("timed out"));
}

#[test]
fn test_is_autobackgrounding_allowed_excludes_sleep() {
    use crate::tools::bash_advanced::is_autobackgrounding_allowed;
    assert!(!is_autobackgrounding_allowed("sleep 10"));
    assert!(is_autobackgrounding_allowed("npm run build"));
    assert!(is_autobackgrounding_allowed("cargo test"));
}

/// R6-T17: when ctx.abort fires mid-execution, the child process is
/// killed and the tool returns a cancellation error. Previously
/// executor-level cancel dropped the Bash future but left the shell
/// child orphaned. Regression guard.
#[tokio::test]
async fn test_bash_cancel_kills_child_and_returns_cancelled() {
    let mut ctx = ToolUseContext::test_default();
    let cancel = tokio_util::sync::CancellationToken::new();
    ctx.abort = coco_tool_runtime::ToolAbortSignal::from_turn(
        coco_tool_runtime::TurnAbortSignal::from_token(cancel.clone()),
    );

    // Fire cancel after 200ms; the command tries to sleep 10s.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();
    });

    let start = std::time::Instant::now();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "sleep 10 && echo done"}),
        &ctx,
    )
    .await;

    let elapsed = start.elapsed();
    // Should return well before 10s — ideally ~200ms plus shell startup.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "cancel should kill child promptly; elapsed={elapsed:?}"
    );
    assert!(matches!(
        result,
        Err(coco_tool_runtime::ToolError::Cancelled)
    ));
}

/// R5-T14: structured output schema — regression guard. Requires
/// stdout/stderr/exitCode/interrupted.
#[tokio::test]
async fn test_bash_structured_output_schema() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "echo out; echo err >&2; exit 2"}),
        &ctx,
    )
    .await
    .unwrap();

    assert!(result.data["stdout"].is_string());
    assert!(result.data["stderr"].is_string());
    assert!(result.data["exitCode"].is_number());
    assert!(result.data["interrupted"].is_boolean());
    assert!(result.data["stdout"].as_str().unwrap().contains("out"));
    assert!(result.data["stderr"].as_str().unwrap().contains("err"));
    assert_eq!(result.data["exitCode"], 2);
    assert_eq!(result.data["interrupted"], false);
}

#[tokio::test]
async fn test_bash_background_without_task_handle() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        json!({"command": "echo test", "run_in_background": true}),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not available"));
}

// -- Stall detection tests --

#[test]
fn test_stall_prompt_yes_no() {
    assert!(coco_tasks::matches_interactive_prompt(
        "Do you want to continue? (y/n)"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "output\nmore output\nContinue? [y/n]"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "Are you sure? (yes/no)"
    ));
}

#[test]
fn test_stall_prompt_password() {
    assert!(coco_tasks::matches_interactive_prompt("Enter password:"));
    assert!(coco_tasks::matches_interactive_prompt(
        "[sudo] password for user:"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "Enter passphrase for key:"
    ));
}

#[test]
fn test_stall_prompt_question_pattern() {
    assert!(coco_tasks::matches_interactive_prompt(
        "Do you want to proceed?"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "Would you like to overwrite?"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "Are you sure you want to delete?"
    ));
}

#[test]
fn test_stall_prompt_press_key() {
    assert!(coco_tasks::matches_interactive_prompt(
        "Press any key to continue"
    ));
    assert!(coco_tasks::matches_interactive_prompt(
        "Press Enter to continue"
    ));
}

#[test]
fn test_stall_no_false_positive_normal_output() {
    // Normal command output should NOT match
    assert!(!coco_tasks::matches_interactive_prompt(
        "Compiling project..."
    ));
    assert!(!coco_tasks::matches_interactive_prompt("Build succeeded"));
    assert!(!coco_tasks::matches_interactive_prompt(
        "Downloaded 42 packages"
    ));
    assert!(!coco_tasks::matches_interactive_prompt("")); // empty
}

#[test]
fn test_stall_only_checks_last_line() {
    // "password:" in earlier output should not trigger
    let tail = "checking password: ok\nall tests passed\nDone.";
    assert!(!coco_tasks::matches_interactive_prompt(tail));

    // But if last line has prompt, it should match
    let tail2 = "checking things\nEnter password:";
    assert!(coco_tasks::matches_interactive_prompt(tail2));
}

// -- Notification format tests --
//
// The XML builder lives in `coco_tasks::notification` and has its own
// unit tests in that crate. These crate-local tests serve as smoke
// checks that the integration path (BashTool spawn → TaskRuntime →
// CommandQueueNotificationSink → render_notification) still produces
// the expected shape.

#[test]
fn test_task_notification_format() {
    use coco_tasks::{NotificationKind, TaskNotification, TerminalStatus, render_notification};
    let n = TaskNotification {
        task_id: "task-1".into(),
        tool_use_id: Some("tu-123".into()),
        agent_id: None,
        output_file: "/tmp/task-1.out".into(),
        description: "ls".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Completed,
            exit_code: Some(0),
        },
    };
    let xml = render_notification(&n);
    assert!(xml.contains("<task-id>task-1</task-id>"));
    assert!(xml.contains("<status>completed</status>"));
    assert!(xml.contains("<tool-use-id>tu-123</tool-use-id>"));
    assert!(xml.contains("<output-file>/tmp/task-1.out</output-file>"));
}

#[test]
fn test_stall_notification_omits_status() {
    use coco_tasks::{NotificationKind, TaskNotification, render_notification};
    let n = TaskNotification {
        task_id: "task-2".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/task-2.out".into(),
        description: "sleep".into(),
        kind: NotificationKind::Stall {
            output_tail: "Enter password:".into(),
        },
    };
    let xml = render_notification(&n);
    assert!(!xml.contains("<status>"));
    assert!(xml.contains("<task-id>task-2</task-id>"));
    assert!(xml.contains("Enter password:"));
}

// ── R7-T11: _simulatedSedEdit short-circuit tests ──
//
// When the BashTool input includes `_simulatedSedEdit: { filePath, newContent }`,
// the tool skips bash entirely and writes the precomputed content to the file
// while preserving its original encoding + line endings, returning a
// sed-shaped result envelope. The tests exercise success, ENOENT,
// and the encoding/line-ending preservation that distinguishes this
// path from FileWriteTool's LF-always policy.

#[tokio::test]
async fn test_bash_simulated_sed_edit_writes_new_content() {
    use crate::tools::bash::BashTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sample.txt");
    std::fs::write(&file, "before\n").unwrap();

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            // Production sed-edit input always carries the original `sed -i …`
            // command alongside the TUI-injected payload (schema requires it).
            "command": "sed -i 's/before/after/' file",
            "_simulatedSedEdit": {
                "filePath": file.to_str().unwrap(),
                "newContent": "after\n",
            }
        }),
        &ctx,
    )
    .await
    .unwrap();

    // sed-shaped success envelope
    assert_eq!(result.data["stdout"], "");
    assert_eq!(result.data["stderr"], "");
    assert_eq!(result.data["exitCode"], 0);
    assert_eq!(result.data["interrupted"], false);

    // File should have the new content on disk.
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert_eq!(on_disk, "after\n");
}

#[tokio::test]
async fn test_bash_simulated_sed_edit_enoent_returns_sed_error() {
    use crate::tools::bash::BashTool;

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "sed -i 's/x/y/' /this/path/does/not/exist.txt",
            "_simulatedSedEdit": {
                "filePath": "/this/path/does/not/exist.txt",
                "newContent": "irrelevant",
            }
        }),
        &ctx,
    )
    .await
    .unwrap();

    // ENOENT must come back as a sed-shaped error envelope, NOT a tool error.
    assert_eq!(result.data["stdout"], "");
    assert_eq!(result.data["exitCode"], 1);
    assert!(
        result.data["stderr"]
            .as_str()
            .unwrap()
            .contains("No such file or directory"),
        "expected sed-style ENOENT, got: {:?}",
        result.data["stderr"]
    );
}

#[tokio::test]
async fn test_bash_simulated_sed_edit_preserves_crlf_line_endings() {
    use crate::tools::bash::BashTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crlf.txt");
    // Original file uses CRLF line endings — simulating a Windows-authored file.
    std::fs::write(&file, "alpha\r\nbeta\r\n").unwrap();

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let _result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "sed -i 's/alpha/gamma/' crlf.txt",
            "_simulatedSedEdit": {
                "filePath": file.to_str().unwrap(),
                "newContent": "gamma\ndelta\n",
            }
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Sed-edit must preserve CRLF — it reuses the detected line ending.
    // coco-rs FileWriteTool always normalizes to LF, so this is the
    // key distinction.
    let on_disk = std::fs::read(&file).unwrap();
    assert!(
        on_disk.windows(2).any(|w| w == b"\r\n"),
        "expected CRLF in output, got: {on_disk:?}"
    );
}

#[tokio::test]
async fn test_bash_simulated_sed_edit_missing_file_path_errors() {
    use crate::tools::bash::BashTool;

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "sed -i 's/a/b/' file",
            "_simulatedSedEdit": {
                "newContent": "no path provided"
            }
        }),
        &ctx,
    )
    .await;

    let err = result.unwrap_err();
    assert!(err.to_string().contains("filePath"), "got: {err}");
}

// ── R7-T12: Bash output schema extension tests ──
//
// Verify that `isImage` and `structuredContent` fields are populated
// correctly. Oversized text output is handled by the generic
// query-level Tool Result Budget pipeline, not by Bash itself.

#[test]
fn test_is_likely_image_bytes_png() {
    use crate::tools::bash::is_likely_image_bytes;
    let png_magic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00];
    assert!(is_likely_image_bytes(&png_magic));
}

#[test]
fn test_is_likely_image_bytes_jpeg() {
    use crate::tools::bash::is_likely_image_bytes;
    assert!(is_likely_image_bytes(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]));
}

#[test]
fn test_is_likely_image_bytes_gif() {
    use crate::tools::bash::is_likely_image_bytes;
    assert!(is_likely_image_bytes(b"GIF89a..."));
    assert!(is_likely_image_bytes(b"GIF87a..."));
}

#[test]
fn test_is_likely_image_bytes_webp() {
    use crate::tools::bash::is_likely_image_bytes;
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&[0, 0, 0, 0]); // size field (don't care)
    webp.extend_from_slice(b"WEBP");
    webp.extend_from_slice(b"VP8 ");
    assert!(is_likely_image_bytes(&webp));
}

#[test]
fn test_is_likely_image_bytes_text_negative() {
    use crate::tools::bash::is_likely_image_bytes;
    assert!(!is_likely_image_bytes(b"hello world\n"));
    assert!(!is_likely_image_bytes(b""));
    // RIFF without WEBP magic isn't an image (could be WAV/AVI).
    assert!(!is_likely_image_bytes(b"RIFF\0\0\0\0WAVEfmt "));
}

// R7-T18: end-to-end image detection now works because coco-shell
// populates `CommandResult.stdout_bytes` with the raw pre-UTF-8-lossy
// bytes. BashTool reads `stdout_bytes` (with `stdout.as_bytes()` as a
// fallback) so the magic-byte detection sees the original PNG header
// instead of the U+FFFD-mangled lossy version.
#[tokio::test]
async fn test_bash_output_includes_image_fields_when_stdout_is_image() {
    use crate::tools::bash::BashTool;

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    // printf with explicit hex: full PNG signature
    // 89 50 4E 47 0D 0A 1A 0A — emitted via printf so the test stays
    // portable across bash versions. The trailing zero bytes are
    // padding to make the magic detector's 8-byte check pass.
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "printf '\\x89PNG\\x0d\\x0a\\x1a\\x0a\\x00\\x00\\x00\\x00'"
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        result.data["isImage"], true,
        "expected isImage=true for PNG-magic stdout, got: {:?}",
        result.data
    );
    let content = result.data["structuredContent"]
        .as_array()
        .expect("structuredContent should be an array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["source"]["media_type"], "image/png");
}

#[tokio::test]
async fn test_bash_output_does_not_use_temp_persistence_when_oversized() {
    use crate::tools::bash::BashTool;

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    // yes(1) emits 'y\n' indefinitely; cap with head -c to land just
    // above the 30K persistence threshold. Some platforms ship bash 3.x
    // without `head -c`, so use `printf` repeats as a portable fallback.
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "printf 'x%.0s' $(seq 1 35000)"
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert!(
        result.data.get("persistedOutputPath").is_none(),
        "Bash should not write model-visible temp persisted output: {:?}",
        result.data
    );
    assert!(
        result.data["stdout"].as_str().unwrap_or("").len() >= 30_000,
        "generic Level 1 needs the full model-visible stdout"
    );
}

#[tokio::test]
async fn test_bash_simulated_sed_edit_does_not_run_command() {
    use crate::tools::bash::BashTool;

    // If the short-circuit is wired correctly, the `command` field should
    // be ignored. We pass a command that would otherwise fail security
    // checks (eval) to prove the bash code path was skipped — the sed
    // path doesn't go through the security pipeline.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("guard.txt");
    std::fs::write(&file, "before\n").unwrap();

    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let result = <BashTool as DynTool>::execute(
        &BashTool,
        serde_json::json!({
            "command": "eval 'rm -rf /'",
            "_simulatedSedEdit": {
                "filePath": file.to_str().unwrap(),
                "newContent": "after\n",
            }
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Sed-shaped success: bash command was never executed.
    assert_eq!(result.data["exitCode"], 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after\n");
}

// ---------------------------------------------------------------------------
// Claude Code hints — model-facing stripping
// ---------------------------------------------------------------------------

#[test]
fn test_maybe_strip_and_record_hints_removes_tag() {
    use crate::tools::bash::maybe_strip_and_record_hints;
    let stdout =
        "line1\n<claude-code-hint v=\"1\" type=\"plugin\" value=\"foo@bar\" />\nline2".to_string();
    let out = maybe_strip_and_record_hints(stdout, "mytool run");
    assert!(
        !out.contains("<claude-code-hint"),
        "hint tag must be stripped from model-visible stdout: {out:?}"
    );
    assert!(out.contains("line1") && out.contains("line2"));
}

#[test]
fn test_maybe_strip_and_record_hints_passthrough_when_no_tag() {
    use crate::tools::bash::maybe_strip_and_record_hints;
    let stdout = "ordinary output\nno tags".to_string();
    let out = maybe_strip_and_record_hints(stdout.clone(), "tool");
    assert_eq!(out, stdout);
}

// ---------------------------------------------------------------------------
// render_for_model
// ---------------------------------------------------------------------------

mod render_for_model_tests {
    use super::*;
    use coco_tool_runtime::DynTool;
    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    #[test]
    fn explicit_background_emits_task_id_and_output_path() {
        // Explicit `run_in_background: true` returns `{backgroundTaskId,
        // outputPath}` (no stdout). render_for_model must name both the task
        // id and the output path so the model can `Read` the file directly
        // (mirrors TS BashTool — replaces the deprecated TaskOutput tool).
        let data = json!({
            "backgroundTaskId": "task-42",
            "outputPath": "/cfg/cache/tasks/sess/task-42.output",
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        assert_eq!(parts.len(), 1);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part, got {:?}", parts[0]);
        };
        assert_eq!(
            text,
            "Command running in background with ID: task-42. Output is being written to: /cfg/cache/tasks/sess/task-42.output"
        );
    }

    #[test]
    fn structured_content_image_decodes_to_filedata_part() {
        // When stdout was an image, `structuredContent` carries a single
        // image block. render_for_model must convert it to FileData so
        // multimodal-capable providers (Anthropic, Gemini 3+) see the
        // raw bytes instead of a base64 string.
        let data = json!({
            "stdout": "(binary image data)",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
            "isImage": true,
            "structuredContent": [{
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "iVBORw0KGgo...",
                }
            }],
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ToolResultContentPart::FileData {
                data,
                media_type,
                filename,
                ..
            } => {
                assert_eq!(data, "iVBORw0KGgo...");
                assert_eq!(media_type, "image/png");
                assert!(filename.is_none());
            }
            other => panic!("expected FileData, got {other:?}"),
        }
    }

    #[test]
    fn text_path_strips_leading_blank_lines() {
        let data = json!({
            "stdout": "  \n\n   \nactual content\n",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        // Leading whitespace-only lines stripped; trailing newline trimmed.
        assert_eq!(text, "actual content");
    }

    #[test]
    fn text_path_includes_stderr_when_present() {
        let data = json!({
            "stdout": "ok",
            "stderr": "warning: bad input",
            "exitCode": 0,
            "interrupted": false,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "ok\nwarning: bad input");
    }

    #[test]
    fn interrupted_appends_error_marker_and_keeps_stderr() {
        let data = json!({
            "stdout": "partial",
            "stderr": "halt",
            "exitCode": -1,
            "interrupted": true,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.contains("partial"), "got: {text}");
        assert!(text.contains("halt"), "got: {text}");
        assert!(
            text.contains("<error>Command was aborted before completion</error>"),
            "got: {text}"
        );
    }

    #[test]
    fn persisted_output_fields_are_ignored_by_model_renderer() {
        // Legacy temp-dir fields are not a model-visible persistence
        // source. The query-level generic Level 1 pipeline owns the
        // <persisted-output> envelope.
        let data = json!({
            "stdout": "(short preview)",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
            "persistedOutputPath": "/tmp/coco-bash-output/bash-1-2.out",
            "persistedOutputSize": 50_000,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.contains("(short preview)"));
        assert!(!text.contains("<persisted-output>"), "got: {text}");
    }

    #[test]
    fn background_task_id_assistant_auto_uses_budget_message() {
        // When the fg→bg auto-promotion fires (assistantAutoBackgrounded),
        // the model sees a verbose message that names the blocking budget so
        // it learns to delegate next time. The default short message is wrong here.
        let data = json!({
            "stdout": "",
            "stderr": "",
            "exitCode": -1,
            "interrupted": true,
            "backgroundTaskId": "task-99",
            "outputPath": "/cfg/cache/tasks/sess/task-99.output",
            "assistantAutoBackgrounded": true,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.contains("<error>Command was aborted"), "got: {text}");
        assert!(
            text.contains("Command exceeded the assistant-mode blocking budget (15s)"),
            "got: {text}"
        );
        assert!(text.contains("task-99"), "got: {text}");
        assert!(
            text.contains("Output is being written to: /cfg/cache/tasks/sess/task-99.output"),
            "got: {text}"
        );
        assert!(text.contains("delegate long-running work"), "got: {text}");
    }

    #[test]
    fn background_task_id_user_initiated_uses_manual_message() {
        // `backgroundedByUser` branch — Ctrl+B path. Coco-rs's TUI doesn't
        // yet wire the keystroke, but the renderer already keys on the
        // `backgroundedByUser` field so adding the keybinding is data-only.
        let data = json!({
            "stdout": "running\n",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
            "backgroundTaskId": "task-7",
            "outputPath": "/cfg/cache/tasks/sess/task-7.output",
            "backgroundedByUser": true,
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(
            text.contains(
                "Command was manually backgrounded by user with ID: task-7. Output is being written to: /cfg/cache/tasks/sess/task-7.output"
            ),
            "got: {text}"
        );
    }

    #[test]
    fn background_task_id_default_uses_short_message() {
        // Default path (`run_in_background: true` issued by the model)
        // — short message; no budget mention.
        let data = json!({
            "stdout": "",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
            "backgroundTaskId": "task-3",
            "outputPath": "/cfg/cache/tasks/sess/task-3.output",
        });
        let parts = <BashTool as DynTool>::render_for_model(&BashTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(
            text.contains(
                "Command running in background with ID: task-3. Output is being written to: /cfg/cache/tasks/sess/task-3.output"
            ),
            "got: {text}"
        );
        assert!(
            !text.contains("blocking budget"),
            "default branch must not mention the budget, got: {text}"
        );
        assert!(
            !text.contains("manually backgrounded"),
            "default branch must not say 'manually backgrounded', got: {text}"
        );
    }

    // `format_byte_size` lives in `shell_render.rs`; its byte-identity
    // contract test lives in `shell_render.test.rs` next to the implementation.
}

// ---------------------------------------------------------------------------
// #34 — head-only output truncation with a lines marker
// ---------------------------------------------------------------------------

#[test]
fn test_truncate_output_is_head_only_with_lines_marker() {
    use super::truncate_output;
    // 10 lines "L0".."L9", each 3 bytes incl newline → 30 bytes total.
    let content: String = (0..10).map(|i| format!("L{i}\n")).collect();
    let out = truncate_output(content.as_bytes(), 9); // keep ~first 3 lines
    // Head retained from the start.
    assert!(out.starts_with("L0\n"), "head should be kept: {out}");
    // No tail half preserved (old behavior kept the end too).
    assert!(
        !out.contains("L9"),
        "tail must be dropped (head-only): {out}"
    );
    // Lines marker, not chars.
    assert!(out.contains("lines truncated"), "got: {out}");
    assert!(!out.contains("chars truncated"), "got: {out}");
}

// ---------------------------------------------------------------------------
// check_permissions seam (shell#162/#164/#167)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bash_check_permissions_jq_danger_asks() {
    // jq system() is no longer treated as read-only; the curated security
    // route sends it to a prompt (#162).
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "jq 'system(\"id\")' data.json"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "jq system() should Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_check_permissions_common_substitution_not_prompted() {
    // The broad substitution analyzers are deliberately NOT routed (they lack
    // safe-substitution carve-outs and would over-prompt). A common
    // `$(...)` in a non-read-only command passes through, not Ask.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "tar czf backup-$(date +%s).tgz src"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Passthrough),
        "common $(...) must not prompt, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_check_permissions_accept_edits_asks_cd_then_write() {
    // TS parity: a non-noop cwd change before a filesystem write requires
    // approval even in acceptEdits mode. `rm`/`mv`/`cp`/`sed` are likewise
    // not blanket-allowed —
    // they route through the dangerous-removal/sed gates and defer to the rule
    // pipeline (acceptEdits auto-accepts file-edit tools, not bash `rm`).
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.mode = coco_types::PermissionMode::AcceptEdits;

    let mkdir = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cd src && mkdir -p out"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(mkdir, coco_types::ToolCheckResult::Ask { .. }),
        "cd + mkdir should Ask in acceptEdits, got {mkdir:?}"
    );

    // A safe `rm` after `cd` still asks under the same cwd+write rule.
    let rm = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cd src && rm old.txt"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(rm, coco_types::ToolCheckResult::Ask { .. }),
        "cd + rm should Ask in acceptEdits, got {rm:?}"
    );
}

#[tokio::test]
async fn test_bash_check_permissions_uses_live_session_cwd_for_noop_cd() {
    let original = tempfile::tempdir().expect("original cwd");
    let live = tempfile::tempdir().expect("live cwd");
    let mut ctx = ToolUseContext::test_default();
    ctx.original_cwd = Some(original.path().to_path_buf());
    ctx.session_cwd = Some(std::sync::Arc::new(tokio::sync::RwLock::new(
        live.path().to_path_buf(),
    )));

    let original_cd_git = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": format!("cd {} && git diff", original.path().display())}),
        &ctx,
    )
    .await;
    assert!(
        matches!(original_cd_git, coco_types::ToolCheckResult::Ask { .. }),
        "cd to original cwd is non-noop when live cwd differs, got {original_cd_git:?}"
    );

    let live_cd_git = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": format!("cd {} && git diff", live.path().display())}),
        &ctx,
    )
    .await;
    assert!(
        matches!(live_cd_git, coco_types::ToolCheckResult::Passthrough),
        "cd to live cwd should be filtered as no-op, got {live_cd_git:?}"
    );
}

// ── Path-constraint gate ──
//
// The 4th bash force-ask gate: an output redirection (or process substitution)
// that writes OUTSIDE the allowed working dirs — or via a shell-expanded /
// unresolvable target — must force Ask and cannot be auto-allowed.

#[tokio::test]
async fn test_bash_redirect_outside_tree_asks() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "echo pwned > /etc/passwd"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "redirect outside the working dirs must Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_redirect_shell_expansion_asks() {
    // A shell-expanded target ($VAR) can't be validated before execution — Ask.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "echo x > $HOME/loot"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "shell-expansion redirect target must Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_redirect_within_tree_passes() {
    // A relative target resolves inside the (process) cwd — no path-gate Ask.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "echo hi > out.txt"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Passthrough),
        "in-tree redirect must not Ask from the path gate, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_redirect_after_cd_asks() {
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cd src && echo hi > out.txt"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "cd + output redirection must Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_redirect_devnull_passes() {
    // /dev/null is always safe.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "echo hi > /dev/null"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Passthrough),
        "redirect to /dev/null must not Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_process_substitution_asks() {
    // `> >(tee …)` writes via a process sub whose target never appears as a
    // redirect target — process substitution forces Ask.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "echo secret > >(tee .git/config)"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "process substitution must Ask, got {result:?}"
    );
}

// ── Per-subcommand write-path gate ──

#[tokio::test]
async fn test_bash_write_outside_tree_asks() {
    // A copy whose destination is outside the working dirs (and not a
    // catastrophic system path the dangerous-removal gate already covers).
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cp secret.txt /opt/exfil/loot"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "write outside the working dirs must Ask, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_write_within_tree_passes() {
    // mkdir of an in-tree dir is not fenced by the write-path gate.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "mkdir newdir"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Passthrough),
        "in-tree write must not Ask from the path gate, got {result:?}"
    );
}

#[tokio::test]
async fn test_bash_read_outside_tree_not_fenced() {
    // Reads are intentionally NOT fenced (coco divergence — avoids noise on
    // routine out-of-cwd inspection). `cat /etc/os-release` passes the gate.
    let ctx = ToolUseContext::test_default();
    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cat /etc/os-release"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Passthrough),
        "out-of-tree read must not Ask, got {result:?}"
    );
}

// ── Out-of-tree write force-ask wiring (G4) ──
//
// A filesystem write whose target is OUTSIDE the allowed working dirs must
// force `Ask` from `check_permissions` (evaluator step-1b), so it cannot be
// auto-allowed by mode / allow-rules. `is_path_within_allowed_dirs` and
// `extract_write_path_targets` are unit-tested elsewhere; this proves the Bash
// tool actually wires them into its permission check.
#[tokio::test]
async fn test_bash_out_of_tree_write_forces_ask() {
    let mut ctx = ToolUseContext::test_default();
    // Pin cwd so the assertion does not depend on the test runner's cwd.
    ctx.cwd_override = Some(std::env::temp_dir());

    let result = <BashTool as DynTool>::check_permissions(
        &BashTool,
        &json!({"command": "cp secret.txt /opt/coco-oob-test/data.txt"}),
        &ctx,
    )
    .await;
    assert!(
        matches!(
            &result,
            coco_types::ToolCheckResult::Ask { message, .. }
                if message.contains("outside the allowed working")
        ),
        "out-of-tree write must force Ask: {result:?}"
    );

    // The Ask must carry an AddDirectories suggestion for the target's parent
    // dir — the only update that unblocks the path gate (it runs before allow
    // rules). This is what lets the TUI render the session/local "always allow"
    // rows instead of collapsing to approve-once/deny. Mirrors TS addDirectories.
    let coco_types::ToolCheckResult::Ask { suggestions, .. } = &result else {
        panic!("expected Ask, got {result:?}");
    };
    assert!(
        matches!(
            suggestions.as_slice(),
            [coco_types::PermissionUpdate::AddDirectories { directories, destination }]
                if directories == &vec!["/opt/coco-oob-test".to_string()]
                    && *destination == coco_types::PermissionUpdateDestination::Session
        ),
        "out-of-tree write Ask must suggest adding the target's parent dir: {suggestions:?}"
    );

    // Negative control: an in-tree read-only command is not gated.
    let benign =
        <BashTool as DynTool>::check_permissions(&BashTool, &json!({"command": "ls -la"}), &ctx)
            .await;
    assert!(
        matches!(benign, coco_types::ToolCheckResult::Passthrough),
        "read-only command must pass through, got {benign:?}"
    );
}
