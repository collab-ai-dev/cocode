//! `/clear` slash command — wipes the local transcript and signals
//! the engine to reset its conversation state. Two-stage handler:
//!
//! ```text
//!  submit("/clear") → take_input → try_local_command (returns false)
//!                                  ↓
//!                                  try_local_clear
//!                                  ↳ do_clear_conversation
//!                                  ↳   messages.clear()
//!                                  ↳   last_agent_markdown = None
//!                                  ↳   overlay = None
//!                                  ↳   toasts.clear() + add cleared toast
//!                                  ↳ command_tx.send(ClearConversation)
//!                                  returns true
//! ```
//!
//! The `ClearConversation` UserCommand reaches the channel; the
//! harness driver discards it (production drives the engine's reset
//! through it, but we don't have `SessionRuntime` wired here). What
//! we *can* assert is the local effect:
//!
//! - Engine transcript is empty after `/clear` (cells wiped).
//! - `session.last_agent_markdown` is `None` (so a subsequent /copy
//!   would correctly toast "no agent response").
//! - Exactly one toast remains — the "cleared conversation" notice.
//! - No new model call (the engine's reset is async + driver-stubbed).

use std::time::Duration;

use anyhow::Result;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text("first reply"), Reply::text("second reply")])
        .with_max_turns(4)
        .build()
        .await?;

    // Build up some history so /clear has something to wipe.
    for prompt in ["one", "two"] {
        harness.submit(prompt).await;
        let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
        assert!(ok, "slash_clear: setup turn `{prompt}` flagged is_error");
    }
    assert_eq!(
        harness.cell_count(),
        4,
        "slash_clear: setup expected 4 cells (U,A,U,A), got {}",
        harness.cell_count(),
    );
    assert!(
        harness.state.session.last_agent_markdown.is_some(),
        "slash_clear: setup expected last_agent_markdown to be set",
    );
    let calls_before_clear = harness.model.call_count();
    assert_eq!(
        calls_before_clear, 2,
        "slash_clear: setup expected 2 LLM calls"
    );

    // Submit /clear — pure local + a fire-and-forget UserCommand.
    harness.submit("/clear").await;

    // Local effect 1: transcript wiped.
    assert!(
        harness.cells_empty(),
        "slash_clear: transcript should be empty after /clear, got {} cells",
        harness.cell_count(),
    );
    // Local effect 2: last_agent_markdown reset (so /copy after /clear
    // surfaces "no agent response", per do_clear_conversation).
    assert!(
        harness.state.session.last_agent_markdown.is_none(),
        "slash_clear: last_agent_markdown should be None after /clear",
    );
    // Local effect 3: exactly one toast remains. do_clear_conversation
    // calls toasts.clear() then adds the "cleared" notice.
    assert_eq!(
        harness.state.ui.toasts.len(),
        1,
        "slash_clear: expected exactly one toast after /clear, got {}",
        harness.state.ui.toasts.len(),
    );

    // Engine effect: NO new model call (the ClearConversation UserCommand
    // is dispatched but the harness driver doesn't act on it).
    assert_eq!(
        harness.model.call_count(),
        calls_before_clear,
        "slash_clear: /clear should not trigger a model call \
         (call_count {} → {})",
        calls_before_clear,
        harness.model.call_count(),
    );

    // After clearing, a fresh prompt should still drive a normal turn.
    // Catches regressions where /clear corrupts internal state and
    // breaks subsequent submits.
    let mut harness = harness;
    harness.events.clear();
    harness.submit("post-clear prompt").await;
    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "slash_clear: post-clear turn flagged is_error");
    assert_eq!(
        harness.model.call_count(),
        3,
        "slash_clear: post-clear submit should bump call_count to 3, got {}",
        harness.model.call_count(),
    );

    harness.shutdown().await;
    Ok(())
}
