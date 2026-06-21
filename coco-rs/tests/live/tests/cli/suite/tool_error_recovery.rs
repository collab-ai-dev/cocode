//! Tool failure recovery: a Bash command whose underlying process exits
//! non-zero must NOT terminate the agent loop. The Bash tool itself
//! completes successfully (it ran the command and captured stderr),
//! the non-zero exit + stderr ride back as the tool result content,
//! and the next turn lets the model react and reply.
//!
//! Engine wiring: `BashTool::execute` returns `Ok(ToolResult)` with the
//! stderr/exit-code embedded — `is_error=false` because the *tool*
//! didn't fail (the process did). `ToolExecutor` feeds the
//! result back into the conversation; the next iteration runs normally.
//!
//! TS parity: tool errors don't terminate the loop in TS either —
//! `query.ts:processToolUse` just appends the result and the next
//! assistant turn proceeds.
//!
//! Distinction from "tool error" (`is_error=true`) — that's reserved
//! for tool-level failures (schema validation, internal panic). A
//! command that simply exited non-zero is *not* a tool error, even
//! though the user might colloquially call it one.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(6),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    // First Bash call hits `cat /no/such/file/here-coco-test` (ENOENT).
    // The model should observe the error and surface it textually.
    let prompt = "Use the Bash tool to run `cat /no/such/file/here-coco-test`. \
                  When it fails, reply with exactly one line: \
                  RESULT=cat-failed-as-expected";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    // At least one Bash tool call must have fired.
    let tool_starts = events::tool_uses_started(&outcome.events);
    assert!(
        tool_starts.contains(&"Bash"),
        "{provider}/{model}: expected at least one Bash tool call; events={}",
        events::summarize(&outcome.events),
    );

    // The agent loop must have continued past the failed command —
    // turns >= 2 means "tool ran, next iteration ran with the result
    // in context". Turn 1 emits the tool call; turn 2 receives the
    // (non-zero exit) result and replies.
    assert!(
        outcome.result.turns >= 2,
        "{provider}/{model}: agent loop must continue after a non-zero exit; \
         turns={} response={:?}",
        outcome.result.turns,
        outcome.result.response_text,
    );

    // Model produced *some* assistant text after the command failure.
    let lower = outcome.result.response_text.to_lowercase();
    assert!(
        !lower.trim().is_empty(),
        "{provider}/{model}: expected non-empty response after non-zero exit; \
         events={}",
        events::summarize(&outcome.events),
    );

    Ok(())
}
