//! `max_turns = 1` boundary: engine must exit cleanly after one
//! iteration even when the model has produced tool calls that would
//! normally keep the loop running.
//!
//! Engine wiring: `BudgetTracker::check` (`app/query/src/budget.rs`)
//! returns `BudgetDecision::Stop { reason: "reached maximum turns ..." }`
//! when `current_turn >= self.max_turns`. The session loop translates
//! that into `cancelled=false, budget_exhausted=true` and exits with
//! `last_continue_reason = None`.
//!
//! TS parity: `query.ts` honours `--max-turns` for headless single-shot
//! execution. coco-rs's CLI suite uses bare-engine harness so no
//! `--max-turns` flag, just `SessionConfig.max_turns`.

use anyhow::Result;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(1),
        // Generous output cap so the first iteration's reply isn't
        // truncated; the boundary we're testing is iteration count, not
        // length.
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    // Ask for a tool call so the model would *want* to take ≥2 turns
    // (turn 1: emit Bash, turn 2: read result + reply). With max_turns=1
    // the engine must stop after iteration 1 instead of looping.
    let prompt = "Run `echo hello-from-bash` via the Bash tool, then summarise \
                  what it printed in one sentence.";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    // Iteration count must be exactly 1.
    assert_eq!(
        outcome.result.turns, 1,
        "expected the engine to exit after 1 iteration; turns={} budget_exhausted={} \
         response={:?}",
        outcome.result.turns, outcome.result.budget_exhausted, outcome.result.response_text,
    );
    assert!(
        outcome.result.budget_exhausted,
        "max_turns=1 should mark budget_exhausted=true when the loop bails because of the cap; \
         got budget_exhausted=false response={:?}",
        outcome.result.response_text,
    );
    assert!(
        !outcome.result.cancelled,
        "max_turns=1 must produce a graceful exit, not cancellation"
    );

    Ok(())
}
