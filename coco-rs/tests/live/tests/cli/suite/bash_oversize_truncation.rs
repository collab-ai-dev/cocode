//! BashTool oversize-output truncation: a command whose stdout exceeds
//! `BashTool::max_result_size_bound()` (30_000) must be truncated /
//! persisted by the engine before the result feeds back into the model,
//! and the agent loop must continue normally afterward.
//!
//! Engine wiring: `coco-tool-runtime::execution` reads
//! `Tool::max_result_size_bound()` and replaces oversized content with a
//! `<persisted-output>` reference (Phase 1 stub: bash-only, parallel
//! JSON fields rather than content replacement). We don't pin the
//! exact replacement format here — only that the loop survives a 100k+
//! char output and the model still produces an assistant reply.
//!
//! Distinct from `tool_error_recovery`: that fires on a non-zero exit;
//! this one fires on a zero-exit command that simply produces too much
//! output. Different code path entirely (size budget vs error path).

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(4),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    // `seq 1 8000` emits ~46k chars (each line is up to 4 digits + `\n`).
    // Comfortably above the 30k Bash cap, well below pathological sizes.
    let prompt = "Use the Bash tool to run exactly: seq 1 8000. \
                  After it returns (the output will be very long), reply with: \
                  RESULT=oversized-output-handled";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    let started = events::tool_uses_started(&outcome.events);
    assert!(
        started.contains(&"Bash"),
        "{provider}/{model}: expected Bash tool call; tool_starts={started:?} events={}",
        events::summarize(&outcome.events),
    );

    // Bash's tool execution is reported as non-error (the *tool* didn't
    // fail; truncation is in-band). The agent loop must continue past
    // the truncated result and produce text.
    let completed = events::tool_uses_completed(&outcome.events);
    let bash_completed = completed.iter().any(|(n, is_err)| *n == "Bash" && !is_err);
    assert!(
        bash_completed,
        "{provider}/{model}: expected non-error Bash completion; \
         tool_completions={completed:?}",
    );
    assert!(
        outcome.result.turns >= 2,
        "{provider}/{model}: agent loop must continue past truncated output; \
         turns={} response={:?}",
        outcome.result.turns,
        outcome.result.response_text,
    );
    assert!(
        !outcome.result.response_text.trim().is_empty(),
        "{provider}/{model}: expected non-empty response after truncation",
    );
    Ok(())
}
