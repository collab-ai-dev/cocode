//! Token usage triple-consistency: `QueryResult.total_usage` (in-memory),
//! `SessionResult.usage` (wire, last protocol notification), and
//! `cost_tracker.total_api_calls` (cost ledger) must all agree on a
//! finished single-turn session.
//!
//! Engine wiring: every API response increments
//! `cost_tracker.record_api_call`, which folds usage into both
//! `total_usage` and the per-model `model_usage` map. The session's
//! final `SessionResult` notification is built from the same ledger
//! via `engine_session::finalize_session_result`. Catches regressions
//! where one of the three sinks falls out of sync (e.g. a refactor
//! that records into the cost tracker but forgets the wire payload).

use anyhow::Result;
use coco_types::CoreEvent;
use coco_types::ServerNotification;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let outcome = run_session(provider, model, SessionConfig::default(), "Reply with: ok").await?;

    let total_usage = &outcome.result.total_usage;
    assert!(
        total_usage.input_tokens.total > 0,
        "{provider}/{model}: expected total_usage.input_tokens.total > 0, got {}",
        total_usage.input_tokens.total,
    );
    assert!(
        total_usage.output_tokens.total > 0,
        "{provider}/{model}: expected total_usage.output_tokens.total > 0, got {}",
        total_usage.output_tokens.total,
    );

    let session_result = outcome.events.iter().rev().find_map(|e| match e {
        CoreEvent::Protocol(ServerNotification::SessionResult(p)) => Some(p),
        _ => None,
    });
    let session_result = session_result.unwrap_or_else(|| {
        panic!(
            "{provider}/{model}: SessionResult missing; events={}",
            events::summarize(&outcome.events)
        )
    });

    assert_eq!(
        total_usage.input_tokens.total, session_result.usage.input_tokens.total,
        "{provider}/{model}: input_tokens mismatch QueryResult vs SessionResult",
    );
    assert_eq!(
        total_usage.output_tokens.total, session_result.usage.output_tokens.total,
        "{provider}/{model}: output_tokens mismatch QueryResult vs SessionResult",
    );

    // SessionResult.num_api_calls is the wire-visible API-call counter;
    // cost_tracker.total_api_calls is the in-memory ledger. They must
    // agree on at least one call (single-turn session).
    let wire_calls = i64::from(session_result.num_api_calls.unwrap_or(0));
    let ledger_calls = outcome.result.cost_tracker.total_api_calls;
    assert_eq!(
        wire_calls, ledger_calls,
        "{provider}/{model}: num_api_calls mismatch wire={wire_calls} ledger={ledger_calls}",
    );
    assert!(
        ledger_calls >= 1,
        "{provider}/{model}: expected cost_tracker.total_api_calls >= 1, got {ledger_calls}",
    );
    Ok(())
}
