//! Mid-flight cancellation. Submits a turn whose first reply is a
//! `Bash` call with a `sleep` long enough that the agent loop is
//! definitely parked inside tool execution; a side task cancels the
//! shared `CancellationToken` after a short delay. The engine watches
//! that token from `tokio::select!` arms in `tool_call_runner`,
//! `permission_controller`, and the main streaming loop — so cancel
//! propagates and `run_session_loop` exits with a cancelled
//! `QueryResult`. `engine_session::run_internal_with_messages` still
//! emits `Idle` + `SessionResult`, so `pump_until_idle` returns.
//!
//! The load-bearing assertion is **wallclock**: the underlying
//! `sleep 1` would take a full second if the engine ignored cancel.
//! We expect the turn to end well under that budget.

use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([
            // Slow tool call. The engine awaits this through its
            // cancellation-aware `tokio::select!`; if cancel doesn't
            // propagate, the test's wallclock check below trips.
            Reply::tool_call(
                "call_sleep_1",
                "Bash",
                json!({
                    "command": "sleep 1",
                    "description": "deliberately slow to be interrupted",
                }),
            ),
            // Defensive: if the test framework re-enters before cancel
            // fires (shouldn't happen), we want a clean stop, not a
            // hang on an empty queue waiting for tool follow-up.
            Reply::stop(),
        ])
        .with_max_turns(4)
        .build()
        .await?;

    let cancel = harness.cancel_token();
    let started = Instant::now();
    harness.submit("run a slow op").await;

    // Side task fires cancel ~80ms in — long enough for the agent
    // loop to have entered tool execution, short enough that wallclock
    // distinguishes "cancelled" from "ran to completion".
    let cancel_for_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        cancel_for_task.cancel();
    });

    // Generous timeout — we're checking that cancel actually short-
    // circuits, not that the engine is fast in general. If cancel
    // doesn't propagate, the engine still completes after the full
    // `sleep 1`, so we'd see ~1s elapsed (still under 2.5s).
    let pump_result = harness.pump_until_idle(Duration::from_millis(2_500)).await;
    let elapsed = started.elapsed();

    pump_result.map_err(|e| anyhow::anyhow!("interrupt_inflight: pump returned error: {e}"))?;

    // Wallclock check — the load-bearing one. With propagation working,
    // we should land well under the slow-op duration. 700ms gives the
    // engine cleanup paths (drain stream, emit Idle/SessionResult,
    // wind down hook forwarder) plenty of headroom while still being
    // diagnostic of "engine ignored cancel" (which would land closer
    // to 1000ms).
    assert!(
        elapsed < Duration::from_millis(700),
        "interrupt_inflight: turn took {:?} — cancellation didn't propagate \
         through the engine's tool-call select",
        elapsed,
    );

    // Tool's completion event should reflect cancellation: the engine
    // surfaces it as an `is_error = true` completion (cancellation
    // maps to `ToolError::Cancelled` on the runtime side).
    let completions = harness.tool_completions();
    assert!(
        completions
            .iter()
            .any(|(name, err)| *name == "Bash" && *err),
        "interrupt_inflight: expected a cancelled Bash completion, got {completions:?}",
    );

    // No second model call — the engine bailed before scheduling the
    // post-tool follow-up. (1 call = the initial tool-emitting reply.)
    assert_eq!(
        harness.model.call_count(),
        1,
        "interrupt_inflight: expected 1 LLM call before cancel, got {}",
        harness.model.call_count(),
    );

    // Don't shutdown — drop runs the same path. Cancel is already
    // fired; explicit shutdown would just re-fire it.
    Ok(())
}
