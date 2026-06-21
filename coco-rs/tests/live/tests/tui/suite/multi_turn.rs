//! Three sequential prompts against a scripted assistant. Verifies
//! state accumulates correctly across turns — each turn appends a user
//! cell, drives one engine round-trip, and lands a fresh assistant
//! text cell without clobbering prior history.

use std::time::Duration;

use anyhow::Result;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([
            Reply::text("turn-1: counted 1"),
            Reply::text("turn-2: counted 2"),
            Reply::text("turn-3: counted 3"),
        ])
        .with_max_turns(4)
        .build()
        .await?;

    let prompts = ["count to 1", "now to 2", "now to 3"];
    let expected_replies = [
        "turn-1: counted 1",
        "turn-2: counted 2",
        "turn-3: counted 3",
    ];

    for (i, prompt) in prompts.iter().enumerate() {
        harness.submit(prompt).await;
        let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
        assert!(ok, "multi_turn: turn {} flagged is_error", i + 1);
    }

    // Three separate engine sessions = three model calls.
    assert_eq!(
        harness.model.call_count(),
        3,
        "multi_turn: expected 3 LLM calls (one per submit), got {}",
        harness.model.call_count()
    );

    // History order: U1, A1, U2, A2, U3, A3 — interleaving is what
    // makes the chat-panel render coherent.
    let text_cells = harness.text_cells_in_order();
    let user_cells: Vec<&str> = text_cells
        .iter()
        .filter_map(|(role, text)| (*role == "user").then_some(*text))
        .collect();
    let assistant_cells: Vec<&str> = text_cells
        .iter()
        .filter_map(|(role, text)| (*role == "assistant").then_some(*text))
        .collect();
    assert_eq!(
        user_cells.len(),
        3,
        "multi_turn: expected 3 user cells, got {} (all cells: {})",
        user_cells.len(),
        text_cells.len()
    );
    assert_eq!(
        assistant_cells.len(),
        3,
        "multi_turn: expected 3 assistant cells, got {}",
        assistant_cells.len()
    );

    for (i, prompt) in prompts.iter().enumerate() {
        assert_eq!(
            user_cells[i], *prompt,
            "multi_turn: user cell {i} text mismatch"
        );
    }
    for (i, expected) in expected_replies.iter().enumerate() {
        assert!(
            assistant_cells[i].contains(expected),
            "multi_turn: assistant {i} text missing `{expected}`, got `{}`",
            assistant_cells[i]
        );
    }

    // turn_count tracks the *current session's* turn number — each
    // `submit` opens a fresh `run_with_events`, so the counter resets
    // to 1 every time. Just confirm we observed at least one
    // TurnCompleted (set above by handle_core_event).
    assert!(
        harness.state.session.turn_count >= 1,
        "multi_turn: turn_count never advanced (={})",
        harness.state.session.turn_count
    );

    harness.shutdown().await;
    Ok(())
}
