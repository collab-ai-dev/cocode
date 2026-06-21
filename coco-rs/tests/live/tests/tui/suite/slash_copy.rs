//! `/copy` slash command — TUI-only side effect, must never reach the
//! agent. The handler chain is:
//!
//! ```text
//!  submit("/copy") → take_input → try_local_command(/copy)
//!                                 ↳ clipboard::copy_last_message
//!                                 ↳ Toast appended
//!                                 returns true
//! ```
//!
//! The branch before the engine ever sees the input. Verifies:
//! - After the slash submission, the model call_count does NOT advance
//!   (no follow-up turn was scheduled).
//! - A toast was appended (success or error — we don't depend on the
//!   real clipboard surface; arboard/OSC52 may either succeed or fail
//!   in a headless test runner, both produce a toast).
//! - `/copy` is not folded into the engine transcript as a user prompt
//!   (it's a TUI-only command, not user content).

use std::time::Duration;

use anyhow::Result;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // First turn: get an assistant reply on the wire so
    // `last_agent_markdown` is set — `/copy` sources from there.
    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text("# heading\n\nbody paragraph")])
        .build()
        .await?;

    harness.submit("hello").await;
    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "slash_copy: setup turn flagged is_error");
    assert_eq!(
        harness.model.call_count(),
        1,
        "slash_copy: setup expected 1 LLM call, got {}",
        harness.model.call_count(),
    );
    assert!(
        harness.state.session.last_agent_markdown.is_some(),
        "slash_copy: last_agent_markdown not set after setup turn — \
         /copy has nothing to source",
    );
    let toasts_before = harness.state.ui.toasts.len();
    let cells_before = harness.cell_count();

    // The slash submission. Must short-circuit at `try_local_command`.
    harness.submit("/copy").await;

    // Engine never re-entered: no new model call.
    assert_eq!(
        harness.model.call_count(),
        1,
        "slash_copy: /copy reached the engine — call_count={}",
        harness.model.call_count(),
    );

    // No new user cell was appended (slash commands aren't user prose).
    assert_eq!(
        harness.cell_count(),
        cells_before,
        "slash_copy: /copy was folded into transcript (delta={})",
        harness.cell_count() - cells_before,
    );
    let saw_slash_text = harness
        .text_cells_in_order()
        .iter()
        .any(|(role, text)| *role == "user" && *text == "/copy");
    assert!(
        !saw_slash_text,
        "slash_copy: `/copy` leaked into transcript as a User cell",
    );

    // Exactly one toast appended (success on systems with clipboard,
    // error toast on headless — both are valid outcomes).
    assert_eq!(
        harness.state.ui.toasts.len(),
        toasts_before + 1,
        "slash_copy: expected exactly one toast appended by /copy, got delta={}",
        harness.state.ui.toasts.len() as i64 - toasts_before as i64,
    );

    // No engine events arrived after the /copy. Use a tiny grace window
    // to catch a regression where SubmitInput leaks downstream.
    let leak = tokio::time::timeout(Duration::from_millis(150), harness.event_rx.recv()).await;
    assert!(
        leak.is_err(),
        "slash_copy: event channel produced an event after /copy \
         (expected silence, got {:?})",
        leak.ok().flatten(),
    );

    harness.shutdown().await;
    Ok(())
}
