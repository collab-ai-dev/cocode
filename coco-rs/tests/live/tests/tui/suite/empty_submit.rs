//! Empty-submit guard: `update::edit::submit` short-circuits when the
//! input buffer is empty — no `UserCommand::SubmitInput` is sent, no
//! user cell is appended, the engine never runs. Belt-and-suspenders
//! defense against accidental Enter on an idle prompt.
//! Verifies all three:
//!
//! - Engine transcript stays empty (no phantom user cell).
//! - `state.ui.input.text` stays empty (nothing to flush).
//! - The event channel stays silent for a short grace window —
//!   the engine should not have been invoked.
//! - `ScriptedModel::call_count() == 0` confirms `do_generate` /
//!   `do_stream` was never reached.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Stock the queue with a reply we expect to *never* be consumed.
    // If a future regression causes empty-submit to send to the engine,
    // the model would be called and `call_count` would jump to 1.
    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text("must-not-fire")])
        .build()
        .await?;

    // Path 1: `submit("")` (the harness helper — same `handle_command`
    // entry the production TUI uses).
    harness.submit("").await;
    assert!(
        harness.cells_empty(),
        "empty_submit: transcript should stay empty after empty submit, \
         got {} cells",
        harness.cell_count(),
    );

    // Path 2: a real Enter keystroke on an empty buffer.
    let enter_changed = harness.press_key(KeyCode::Enter, KeyModifiers::NONE).await;
    // The handler still returns `true` (the dispatcher claims the key
    // even if the buffer was empty) but the side-effects below must
    // all be absent.
    let _ = enter_changed;
    assert!(
        harness.cells_empty(),
        "empty_submit: empty Enter should not append a user cell, \
         got {} cells",
        harness.cell_count(),
    );
    assert_eq!(
        harness.state.ui.input.text(),
        "",
        "empty_submit: input buffer should still be empty after Enter, \
         got {:?}",
        harness.state.ui.input.text(),
    );

    // The engine should not have produced any events. Wait briefly to
    // catch a regression where SubmitInput leaks through and an entire
    // SessionStarted/Result chain lands.
    let leak = tokio::time::timeout(Duration::from_millis(150), harness.event_rx.recv()).await;
    assert!(
        leak.is_err(),
        "empty_submit: event channel produced an event for an empty submit \
         — the engine ran when it shouldn't have (event={:?})",
        leak.ok().flatten(),
    );
    assert_eq!(
        harness.model.call_count(),
        0,
        "empty_submit: scripted model was called {}× — expected 0",
        harness.model.call_count(),
    );

    // Render still produces a valid frame even with no chat history —
    // catches "empty messages list panics the chat-panel widget".
    let rendered = harness.render_to_string()?;
    assert!(
        !rendered.trim().is_empty(),
        "empty_submit: render produced an all-blank buffer with empty state",
    );

    // Shutdown without `pump_until_idle` — there's nothing to pump.
    let _ = tokio::time::timeout(Duration::from_secs(2), harness.shutdown()).await;
    Ok(())
}
