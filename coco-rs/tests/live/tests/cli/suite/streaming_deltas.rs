//! Streaming text deltas: assert `AgentStreamEvent::TextDelta` events
//! arrive incrementally during a normal text turn — not just at the end.
//!
//! Engine wiring: provider streams emit `StreamEvent::TextDelta` chunks;
//! `engine.rs` re-emits them as `AgentStreamEvent::TextDelta` and the
//! `StreamAccumulator` flushes them as `agentMessage/delta` over the
//! wire. The TS path produces multiple deltas for any non-trivial reply;
//! coco-rs must too. Catches "streaming pipeline silently dropping
//! deltas / emitting only on completion" regressions.
//!
//! Distinct from `one_shot`: that test only inspects `response_text`
//! (the concatenated final string) — a backend that buffers the entire
//! response and emits a single delta would still pass it. This test
//! requires `>= 2` TextDelta events, which proves the chunked path is
//! actually being driven.

use anyhow::Result;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    // Long-ish reply ensures the provider emits multiple chunks.
    let prompt = "Recite the first six lines of a well-known nursery rhyme \
                  (Mary Had a Little Lamb). Output the lines verbatim, one per \
                  line, with no commentary.";
    let outcome = run_session(provider, model, SessionConfig::default(), prompt).await?;

    let delta_count = outcome
        .events
        .iter()
        .filter(|e| matches!(e, CoreEvent::Stream(AgentStreamEvent::TextDelta { .. })))
        .count();

    assert!(
        delta_count >= 2,
        "{provider}/{model}: expected >=2 TextDelta events from streaming, got {delta_count}; \
         events={}",
        events::summarize(&outcome.events),
    );
    assert!(
        !outcome.result.response_text.trim().is_empty(),
        "{provider}/{model}: response_text was empty; events={}",
        events::summarize(&outcome.events),
    );
    Ok(())
}
