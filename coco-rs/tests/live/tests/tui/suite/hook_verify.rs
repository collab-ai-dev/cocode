//! Hook-driven functional verification.
//!
//! Registers a `PreToolUse` and a `PostToolUse` Command-style hook that
//! each append a single line to a tracefile in the harness workdir.
//! Then asks the model to issue a `Bash` tool call. After the engine
//! finishes, the tracefile is read back to prove:
//!
//! - Both hooks fired (presence of the line).
//! - They fired in the right order (`PreToolUse` strictly before
//!   `PostToolUse`).
//! - The hook environment carried the tool name correctly via the
//!   `$HOOK_TOOL_NAME` env var (the same channel TS hooks read from).
//!
//! This pattern matches how a real user would write a hook (a small
//! shell command that records or transforms state) — so the test
//! doubles as documentation for hook authors.

use std::time::Duration;

use anyhow::Result;
use coco_hooks::HookDefinition;
use coco_hooks::HookHandler;
use coco_types::HookEventType;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Trace path lives inside the workdir we hand to the harness, so
    // the absolute path can be baked into both hooks AND the assertion
    // path. Pre-mint the workdir to satisfy that order.
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-hook-")
        .tempdir_in("/tmp")?;
    let trace_path = workdir.path().join("hook-trace.log");
    let trace_str = trace_path.to_string_lossy().into_owned();

    // The two hook commands. They use env vars the orchestrator
    // injects (`HOOK_TOOL_NAME` / `HOOK_EVENT` per
    // `coco_hooks::orchestration::build_hook_env`); redirecting
    // stdout-of-`echo` is sufficient because hooks run via `sh -c` by
    // default.
    let pre_hook = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: format!("echo PRE:$HOOK_TOOL_NAME >> {trace_str}"),
            timeout_ms: Some(5_000),
            shell: None,
        },
        priority: 0,
        scope: Default::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: Some("recording pre-tool trace".into()),
    };
    let post_hook = HookDefinition {
        event: HookEventType::PostToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: format!("echo POST:$HOOK_TOOL_NAME >> {trace_str}"),
            timeout_ms: Some(5_000),
            shell: None,
        },
        priority: 0,
        scope: Default::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: Some("recording post-tool trace".into()),
    };

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_hooks([pre_hook, post_hook])
        .with_replies([
            Reply::text_then_tool(
                "Running a quick echo to verify the hooks.",
                "call_bash_hook_1",
                "Bash",
                json!({
                    "command": "echo hook-test",
                    "description": "trace echo",
                }),
            ),
            Reply::text("hooks recorded the bash call"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("run echo hook-test").await;
    let ok = harness.pump_until_idle(Duration::from_secs(20)).await?;
    assert!(ok, "hook_verify: SessionResult flagged is_error");

    // The Bash tool ran exactly once.
    let starts = harness.tool_starts();
    assert_eq!(
        starts,
        vec!["Bash"],
        "hook_verify: expected single Bash tool, got {starts:?}"
    );

    // Read the tracefile back. Hooks fire synchronously (is_async =
    // false), so by the time SessionResult landed both lines must be
    // on disk.
    let trace = std::fs::read_to_string(&trace_path)
        .map_err(|e| anyhow::anyhow!("read trace {}: {e}", trace_path.display()))?;
    let lines: Vec<&str> = trace.lines().filter(|l| !l.is_empty()).collect();

    let pre_idx = lines
        .iter()
        .position(|l| l.trim() == "PRE:Bash")
        .ok_or_else(|| {
            anyhow::anyhow!("hook_verify: `PRE:Bash` trace line missing — got lines:\n{trace}")
        })?;
    let post_idx = lines
        .iter()
        .position(|l| l.trim() == "POST:Bash")
        .ok_or_else(|| {
            anyhow::anyhow!("hook_verify: `POST:Bash` trace line missing — got lines:\n{trace}")
        })?;

    assert!(
        pre_idx < post_idx,
        "hook_verify: PreToolUse must fire before PostToolUse \
         (pre={pre_idx}, post={post_idx})\n{trace}"
    );

    harness.shutdown().await;
    Ok(())
}
