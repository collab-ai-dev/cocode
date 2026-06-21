//! PreToolUse hook intercept — proves the production hook orchestration
//! fired when the real model issued a tool call.
//!
//! We install a hook via the prod settings.json path (the same path
//! `SessionRuntime::build` reads). The hook is a shell command that
//! writes a marker file when fired. After the agent runs Bash, we
//! assert the marker file exists — that's load-bearing evidence that
//! the orchestrator (`coco_hooks::orchestration`) ran in the real
//! engine flow, not just in unit tests.

use std::time::Duration;

use anyhow::Result;
use coco_types::HookEventType;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    // Pre-mint the workdir so we can compute the absolute marker path
    // before the harness boots and write it into the settings.json
    // hook command.
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-hook-")?;
    let workdir_path = workdir.path().to_path_buf();
    let marker_path = workdir_path.join("hook-fired.txt");
    let marker_str = marker_path.to_string_lossy().into_owned();

    // settings.json — installs a PreToolUse hook on Bash. Same shape
    // production users write in `~/.coco/settings.json`. We escape the
    // shell to handle paths with spaces, but tempdirs under /tmp are
    // ASCII-safe so it's redundant — kept for clarity.
    let settings_path = workdir_path.join("settings.json");
    let settings_body = serde_json::json!({
        "hooks": {
            HookEventType::PreToolUse.as_str(): [{
                "type": "command",
                "matcher": "Bash",
                "command": format!("echo fired > '{marker_str}'"),
                "timeout": 5,
            }]
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&settings_body).expect("settings.json"),
    )?;

    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(6)
        .with_settings_path(settings_path)
        .with_workdir(workdir)
        .build()
        .await?;

    let ws = harness.workdir().to_string_lossy().into_owned();
    let prompt = format!(
        "Use the Bash tool to print exactly the literal string `BANANA` via \
         `printf BANANA`. Do not write any files. Do not use other tools. \
         Then reply with `done` and nothing else. CWD: `{ws}`."
    );

    harness.submit(&prompt).await;

    let ok = harness.pump_until_idle(Duration::from_secs(60)).await?;
    assert!(ok, "{provider}/{model}: SessionResult flagged is_error");

    // The model SHOULD have run Bash at least once.
    let starts = harness.tool_starts();
    assert!(
        starts.contains(&"Bash"),
        "{provider}/{model}: expected ≥1 Bash invocation; tool starts = {starts:?}",
    );

    // Load-bearing assertion: the hook's side-effect on disk.
    assert!(
        marker_path.exists(),
        "{provider}/{model}: PreToolUse hook should have written {} \
         (hook orchestrator did not fire on real Bash invocation)",
        marker_path.display(),
    );
    let body = std::fs::read_to_string(&marker_path)?;
    assert!(
        body.contains("fired"),
        "{provider}/{model}: hook marker has unexpected body: {body:?}",
    );

    harness.shutdown().await;
    Ok(())
}
