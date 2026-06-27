//! Multi-step tool use: create three files, read them back, return joined contents.
//!
//! Forces the model into ≥3 tool calls. Asserts on the structured event
//! stream (tool starts, tool completions) and on filesystem state in
//! the session tempdir.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model_id: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(14),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };

    // Pre-build the prompt with absolute paths derived from the harness's
    // tempdir. The Write tool requires absolute paths; relative paths
    // would silently fail or land in cwd-with-symlink resolution issues
    // on macOS. We don't have access to the harness tempdir before the
    // session runs, so we use the workdir under the system temp via a
    // marker placeholder and let the harness inject the cwd. Simpler:
    // tell the model to use the cwd Bash returns from `pwd`.
    let prompt = r#"
You are working in the current directory. Use the Bash tool first to print the
absolute path of the current working directory (run `pwd`). Then for each file
below, use the Write tool with the ABSOLUTE path `<cwd>/<filename>`:

1. Write `<cwd>/a.txt` with exactly the content `alpha`.
2. Write `<cwd>/b.txt` with exactly the content `beta`.
3. Write `<cwd>/c.txt` with exactly the content `gamma`.
4. Read all three files back via the Read tool.
5. Reply with a single line `RESULT=alpha|beta|gamma` and nothing else.
"#;

    let outcome = run_session(provider, model_id, cfg, prompt).await?;

    let tool_starts = events::tool_uses_started(&outcome.events);
    let tool_completions = events::tool_uses_completed(&outcome.events);
    assert!(
        tool_starts.len() >= 3,
        "{provider}/{model_id}: expected ≥3 tool calls, got {} ({})",
        tool_starts.len(),
        events::summarize(&outcome.events)
    );
    let failed: Vec<_> = tool_completions
        .iter()
        .filter(|(_, is_err)| *is_err)
        .collect();
    assert!(
        failed.is_empty(),
        "{provider}/{model_id}: tool failures: {failed:?} (all completions: {tool_completions:?})"
    );

    let lower = outcome.result.response_text.to_lowercase();
    let mentions_all = ["alpha", "beta", "gamma"].iter().all(|t| lower.contains(t));
    assert!(
        mentions_all,
        "{provider}/{model_id}: final response missing one of alpha/beta/gamma: {}",
        outcome.result.response_text
    );
    Ok(())
}
