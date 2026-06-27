//! Single-prompt round trip with a scripted assistant. Verifies:
//! - The harness pumps the engine end-to-end (SessionStarted →
//!   TurnStarted → TextDelta(s) → TurnCompleted → SessionResult).
//! - `handle_core_event` folds `TurnCompleted`'s flushed streaming
//!   buffer into an `AssistantText` cell.
//! - The rendered buffer surfaces both the user's prompt and the
//!   assistant's reply (proves the chat panel widget pulled them out
//!   of the engine transcript).

use std::time::Duration;

use anyhow::Result;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text("ack from scripted model: hello back")])
        .build()
        .await?;

    harness.submit("hello").await;
    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(
        ok,
        "one_shot: SessionResult flagged is_error=true ({} events)",
        harness.events.len()
    );

    // Engine round-trip: exactly one model call (no tool follow-ups).
    assert_eq!(
        harness.model.call_count(),
        1,
        "one_shot: expected 1 LLM call, got {}",
        harness.model.call_count()
    );

    // State shape: user message + assistant reply landed in the transcript.
    assert!(
        harness.has_user_cell(),
        "one_shot: user cell not folded into transcript (cell count {})",
        harness.cell_count(),
    );
    assert!(
        harness.assistant_text_contains("ack from scripted model"),
        "one_shot: assistant reply not folded into transcript \
         (cell count {})",
        harness.cell_count(),
    );

    // Render side: user-visible buffer surfaces both turns.
    let rendered = harness.render_to_string()?;
    assert!(
        rendered.contains("hello"),
        "one_shot: rendered buffer missing user prompt:\n{rendered}"
    );
    assert!(
        rendered.contains("ack from scripted model"),
        "one_shot: rendered buffer missing assistant reply:\n{rendered}"
    );
    assert_no_large_gap_between_last_reply_and_input(&rendered);

    harness.shutdown().await;
    Ok(())
}

fn assert_no_large_gap_between_last_reply_and_input(rendered: &str) {
    let lines: Vec<&str> = rendered.lines().collect();
    let assistant_idx = lines
        .iter()
        .position(|line| line.contains("ack from scripted model"))
        .unwrap_or_else(|| panic!("one_shot: assistant line missing:\n{rendered}"));
    let input_idx = lines
        .iter()
        .enumerate()
        .skip(assistant_idx + 1)
        .find_map(|(index, line)| line.trim_start().starts_with("❯").then_some(index))
        .unwrap_or_else(|| panic!("one_shot: input prompt after assistant missing:\n{rendered}"));
    let gap = input_idx.saturating_sub(assistant_idx + 1);
    assert!(
        gap <= 3,
        "one_shot: input viewport drifted {gap} rows below the final assistant reply:\n{rendered}"
    );
}
