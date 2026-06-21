//! `CommandQueue` mid-turn injection: a command enqueued while the
//! engine is mid-turn (e.g. during a tool execution) must be drained
//! into the conversation as a user message and visible to the model
//! on the next turn.
//!
//! Engine wiring: `QueryEngine::command_queue() -> &CommandQueue` is
//! `Arc`-backed, so callers can hold a clone and `enqueue()` from a
//! parallel task. The drain happens at end-of-turn
//! (`engine.rs:drain_command_queue_into_history`) — the queued prompt
//! is appended to history as a fresh user message and the next API
//! call sees it.
//!
//! TS parity: `utils/messageQueueManager.ts` enables exactly this
//! "user types while the LLM is working" UX. The queue is the central
//! steering mechanism — without it, mid-turn input is impossible. This
//! test pins the queue → drain → next-turn-prompt path end-to-end.
//!
//! Test design: turn 1 runs a 3-second Bash sleep. While the engine is
//! waiting on the child, we inject a `Now`-priority command containing
//! a unique marker and a directive to include it in the final reply.
//! After the bash completes, the engine drains the queue, kicks turn 2
//! with the queued message in history, and the model's reply must
//! contain the marker.

use std::time::Duration;

use anyhow::Result;
use coco_query::QueuedCommand;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session_with_steering;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    const MARKER: &str = "QUEUE-MARK-9911";

    let cfg = SessionConfig {
        max_turns: Some(6),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    let prompt = "Use the Bash tool to run exactly: sleep 3 && echo done. \
                  After it returns, follow whatever instruction the user \
                  sends next, then summarise.";

    let outcome = run_session_with_steering(provider, model, cfg, prompt, |queue| {
        tokio::spawn(async move {
            // Wait long enough for the bash child to be spawned and
            // the engine to be parked on the tool. 1.5s is comfortably
            // before the 3s sleep finishes.
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            queue
                .enqueue(QueuedCommand::new(
                    format!(
                        "Append the literal token {MARKER} to your final \
                         reply on its own line, exactly as written."
                    ),
                    coco_query::QueuePriority::Now,
                ))
                .await;
        })
    })
    .await?;

    assert!(
        outcome.result.response_text.contains(MARKER),
        "{provider}/{model}: queued mid-turn command never reached the \
         model — marker {MARKER:?} missing from response. \
         response={:?} events={}",
        outcome.result.response_text,
        events::summarize(&outcome.events),
    );
    // Defence in depth: ensure the loop actually ran multiple turns
    // (so the queued message had a turn to be processed in).
    assert!(
        outcome.result.turns >= 2,
        "{provider}/{model}: expected >=2 turns so injected command can \
         be drained; turns={}",
        outcome.result.turns,
    );
    Ok(())
}
