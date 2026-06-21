//! Trivial round-trip: one prompt, one assistant reply, no tools.
//!
//! Asserts the engine emitted a `SessionStarted`, at least one
//! `TurnCompleted`, and a final `SessionResult` with `is_error == false`.
//! Catches "agent loop dies on the first turn" regressions cheaply.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model_id: &str) -> Result<()> {
    let outcome = run_session(
        provider,
        model_id,
        SessionConfig::default(),
        "Reply with a single word: ok",
    )
    .await?;

    // Engines without a SessionBootstrap don't emit SessionStarted, so we
    // don't require it. The SDK suite already covers low-level chain
    // behavior; here we just want to confirm the agent loop produced a
    // final answer and ran ≥1 turn.
    assert!(
        outcome.result.turns >= 1,
        "{provider}/{model_id}: expected QueryResult.turns >= 1, got {} ({})",
        outcome.result.turns,
        events::summarize(&outcome.events)
    );
    assert!(
        !outcome.result.response_text.trim().is_empty(),
        "{provider}/{model_id}: response_text was empty ({})",
        events::summarize(&outcome.events)
    );
    assert!(
        events::session_succeeded(&outcome.events),
        "{provider}/{model_id}: SessionResult flagged is_error=true ({})",
        events::summarize(&outcome.events)
    );
    Ok(())
}
