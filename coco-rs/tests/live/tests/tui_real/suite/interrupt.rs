//! Interrupt mid-flight — exercises the `UserCommand::Interrupt` path
//! through the real TUI driver against a real LLM.
//!
//! The test asks the agent to start a slow Bash command (`sleep 10`).
//! While the engine is awaiting tool output, the test fires an
//! Interrupt. The driver's active-turn cancel token cascades through
//! every `tokio::select!` arm in the engine; the turn returns and
//! the harness sees `SessionResult` quickly.
//!
//! Two timing properties this proves:
//! 1. The interrupt actually propagates — without it, the test would
//!    hang waiting for `sleep 10` to finish.
//! 2. Cancellation flushes a `SessionResult` through the event channel
//!    so the production TUI's "ready for next prompt" cue still fires
//!    after Esc/Ctrl+C.

use std::time::Duration;

use anyhow::Result;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(4)
        .build()
        .await?;

    let prompt = "Use the Bash tool to run `sleep 10 && echo done`. \
                  When the command completes, reply with `finished`.";
    harness.submit(prompt).await;

    // Wait for the Bash tool to actually start before interrupting,
    // otherwise the cancel might land before the engine entered the
    // tool-execution state and the test wouldn't prove much. We pump
    // events until we see ToolUseStarted, then cancel.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut saw_bash = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, harness.event_rx.recv()).await {
            Ok(Some(evt)) => {
                use coco_types::AgentStreamEvent;
                use coco_types::CoreEvent;
                let started_bash = matches!(
                    &evt,
                    CoreEvent::Stream(AgentStreamEvent::ToolUseStarted { name, .. })
                        if name == "Bash"
                );
                coco_tui::server_notification_handler::handle_core_event(
                    &mut harness.state,
                    evt.clone(),
                    &harness.command_tx,
                );
                harness.events.push(evt);
                if started_bash {
                    saw_bash = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    assert!(
        saw_bash,
        "{provider}/{model}: never saw Bash start before deadline; \
         model may have refused the tool call entirely",
    );

    // Fire the interrupt and time how long it takes for SessionResult
    // to arrive. With `sleep 10` running, an absent-cancel pipeline
    // would block ≥10s; correct cancellation lands well under that.
    let t0 = tokio::time::Instant::now();
    harness.interrupt().await?;

    // Drain remaining events — we don't assert on `is_error` here
    // because the engine may report cancellation either way; what
    // we care about is the *speed* of completion.
    let _ = harness.pump_until_idle(Duration::from_secs(8)).await?;
    let elapsed = t0.elapsed();

    assert!(
        elapsed < Duration::from_secs(8),
        "{provider}/{model}: interrupt did not cancel in time \
         (elapsed = {elapsed:?}); cancel propagation may be broken",
    );

    harness.shutdown().await;
    Ok(())
}
