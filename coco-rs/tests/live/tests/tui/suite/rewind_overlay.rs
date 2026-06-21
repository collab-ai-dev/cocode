//! `/rewind` slash command — opens the rewind overlay populated from
//! the current user-message history. Pure-local TUI mutation; the
//! actual rewind dispatch (`UserCommand::Rewind`) needs `SessionRuntime`
//! we deliberately don't wire here. What we *can* test is the overlay
//! construction:
//!
//! - Bare `/rewind` opens an overlay with one row per selectable user
//!   message **plus** the synthetic "current prompt" anchor row that
//!   `build_rewind_overlay_internal` always appends (TS:
//!   `MessageSelector.tsx:60-66`).
//! - `/rewind 2` pre-selects the 2nd row (1-based → index 1), exercising
//!   the numeric-arg branch in `try_local_command`.
//! - `/rewind last` pre-selects the final row (the synthetic anchor).
//!
//! Verifies the slash command never reaches the engine and the overlay is
//! active after submission.

use std::time::Duration;

use anyhow::Result;
use coco_tui::state::ModalState;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([
            Reply::text("ack one"),
            Reply::text("ack two"),
            Reply::text("ack three"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    // 3 turns → 3 selectable user messages in history.
    for prompt in ["alpha", "beta", "gamma"] {
        harness.submit(prompt).await;
        let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
        assert!(ok, "rewind_overlay: setup turn `{prompt}` flagged is_error");
    }
    let calls_before = harness.model.call_count();
    assert_eq!(
        calls_before, 3,
        "rewind_overlay: setup expected 3 LLM calls, got {calls_before}",
    );
    assert!(
        harness.state.ui.modal.is_none(),
        "rewind_overlay: modal should be None before /rewind",
    );

    // Variant 1: bare /rewind. Overlay opens with 4 rows = 3 real + 1
    // synthetic anchor. Default selection is the synthetic anchor (so
    // Enter dismisses without rewinding — TS parity).
    harness.submit("/rewind").await;
    let overlay = match harness.state.ui.modal.as_ref() {
        Some(ModalState::Rewind(r)) => r.clone(),
        other => panic!("rewind_overlay: /rewind should set ModalState::Rewind, found {other:?}"),
    };
    assert_eq!(
        overlay.messages.len(),
        4,
        "rewind_overlay: expected 3 user msgs + 1 synthetic anchor = 4 rows, \
         got {} (rows={:?})",
        overlay.messages.len(),
        overlay
            .messages
            .iter()
            .map(|m| &m.display_text)
            .collect::<Vec<_>>(),
    );
    assert!(
        overlay
            .messages
            .iter()
            .filter(|m| !m.is_current_prompt)
            .count()
            == 3,
        "rewind_overlay: expected 3 non-anchor rows",
    );
    let anchor_idx = overlay
        .messages
        .iter()
        .position(|m| m.is_current_prompt)
        .expect("rewind_overlay: synthetic anchor row missing");
    assert_eq!(
        anchor_idx, 3,
        "rewind_overlay: synthetic anchor should be the last row",
    );
    let real_texts: Vec<&str> = overlay
        .messages
        .iter()
        .filter(|m| !m.is_current_prompt)
        .map(|m| m.display_text.as_str())
        .collect();
    assert_eq!(
        real_texts,
        vec!["alpha", "beta", "gamma"],
        "rewind_overlay: rows should preserve user-message order",
    );

    // Variant 2: /rewind 2 — pre-selects index 1 (the second user message).
    // Reset the overlay to a fresh state to exercise the arg branch in
    // isolation.
    harness.state.ui.clear_surfaces();
    harness.submit("/rewind 2").await;
    let overlay = match harness.state.ui.modal.as_ref() {
        Some(ModalState::Rewind(r)) => r.clone(),
        other => panic!("rewind_overlay: /rewind 2 should set ModalState::Rewind, found {other:?}"),
    };
    assert_eq!(
        overlay.selected, 1,
        "rewind_overlay: /rewind 2 should pre-select index 1 (got {})",
        overlay.selected,
    );

    // Variant 3: /rewind last — pre-selects the final row (synthetic anchor).
    harness.state.ui.clear_surfaces();
    harness.submit("/rewind last").await;
    let overlay = match harness.state.ui.modal.as_ref() {
        Some(ModalState::Rewind(r)) => r.clone(),
        other => {
            panic!("rewind_overlay: /rewind last should set ModalState::Rewind, found {other:?}")
        }
    };
    assert_eq!(
        overlay.selected,
        (overlay.messages.len() as i32).saturating_sub(1),
        "rewind_overlay: /rewind last should pre-select the final row \
         (got {} of {})",
        overlay.selected,
        overlay.messages.len(),
    );

    // Engine effect: none of the three /rewind variants triggered a model call.
    assert_eq!(
        harness.model.call_count(),
        calls_before,
        "rewind_overlay: /rewind shouldn't trigger a model call \
         (call_count {} → {})",
        calls_before,
        harness.model.call_count(),
    );

    // History stayed intact — /rewind only opens an overlay, the actual
    // truncation happens later when the user picks a target.
    let user_count = harness
        .text_cells_in_order()
        .iter()
        .filter(|(role, _)| *role == "user")
        .count();
    assert_eq!(
        user_count, 3,
        "rewind_overlay: /rewind should NOT mutate history \
         (got {user_count} user cells, expected 3)",
    );

    harness.shutdown().await;
    Ok(())
}
