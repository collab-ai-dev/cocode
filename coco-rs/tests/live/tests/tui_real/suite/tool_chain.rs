//! Multi-turn tool-chain through the real provider.
//!
//! The agent must run `Bash` (pwd), `Write` (create a file), `Read`
//! (read it back), then summarize — exercising the real model's
//! tool-call extraction, the engine's per-turn tool dispatch, and the
//! AppState fold for tool blocks.
//!
//! Asserts on:
//! - ≥3 distinct tool starts (proves multi-turn agent loop, not a
//!   single all-in-one fabrication)
//! - The marker file the agent wrote actually exists on disk (real
//!   tool side-effect)
//! - The final assistant text references the file contents
//! - All tool completions report `is_error == false`

use std::time::Duration;

use anyhow::Result;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(14)
        .build()
        .await?;

    let workdir = harness.workdir();
    let workdir_str = workdir.to_string_lossy().into_owned();
    let prompt = format!(
        "You are working in the cwd `{ws}`. Use tools (no shortcuts):\n\
         1. Use the Bash tool to run `pwd` and confirm the cwd.\n\
         2. Use the Write tool with absolute path `{ws}/marker.txt` to write \
            EXACTLY the content `iguana`.\n\
         3. Use the Read tool to read `{ws}/marker.txt` back.\n\
         4. Reply with `RESULT=<contents>` and nothing else.",
        ws = workdir_str,
    );

    harness.submit(&prompt).await;

    let ok = harness.pump_until_idle(Duration::from_secs(120)).await?;
    assert!(ok, "{provider}/{model}: SessionResult flagged is_error");

    let starts = harness.tool_starts();
    assert!(
        starts.len() >= 3,
        "{provider}/{model}: expected ≥3 tool starts (Bash + Write + Read), got {starts:?}",
    );

    let completions = harness.tool_completions();
    let failed: Vec<_> = completions.iter().filter(|(_, e)| *e).collect();
    assert!(
        failed.is_empty(),
        "{provider}/{model}: tool failures: {failed:?} (all: {completions:?})",
    );

    // Real side-effect on disk — the highest-confidence assertion that
    // the agent actually used the tools rather than fabricating output.
    let written = workdir.join("marker.txt");
    assert!(
        written.exists(),
        "{provider}/{model}: expected {} to exist after agent run",
        written.display(),
    );
    let body = std::fs::read_to_string(&written)?;
    assert!(
        body.contains("iguana"),
        "{provider}/{model}: marker contents unexpected: {body:?}",
    );

    let text = harness.assistant_text().to_lowercase();
    assert!(
        text.contains("iguana"),
        "{provider}/{model}: final reply should reference `iguana`, got {text:?}",
    );

    harness.shutdown().await;
    Ok(())
}
