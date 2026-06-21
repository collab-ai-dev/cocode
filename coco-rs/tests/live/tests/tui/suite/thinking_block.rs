//! Reasoning + final-text turn. Uses `Reply::text_with_thinking` so the
//! scripted stream emits a `Reasoning` block followed by a `Text` block,
//! exercising the engine's reasoning pipeline:
//!
//! ```text
//!  AssistantContentPart::Reasoning  →  Part::Reasoning{Start,Delta,End}
//!  AssistantContentPart::Text       →  Part::Text{Start,Delta,End}
//!                                                    ↓
//!  StreamAccumulator                →  AgentStreamEvent::ThinkingDelta
//!                                       AgentStreamEvent::TextDelta
//!                                                    ↓
//!  TUI handler                      →  state.ui.streaming.{thinking, content}
//!                                                    ↓
//!  TurnCompleted                    →  engine pushes Message::Assistant via
//!                                       MessageAppended → transcript cell
//!                                       (thinking stays in the streaming buffer
//!                                        and is dropped on `take()` — by design,
//!                                        thinking is real-time-only)
//! ```
//!
//! Verifies:
//! - The wire carried a `ThinkingDelta` with the scripted reasoning text.
//! - The final assistant text landed as a cell on the transcript.
//! - `state.ui.streaming` is `None` after the turn (took-and-flushed).

use std::time::Duration;

use anyhow::Result;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let thinking = "weighing options: A is faster, B is safer";
    let body = "going with B for safety";

    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text_with_thinking(thinking, body)])
        .build()
        .await?;

    harness.submit("which is better, A or B?").await;
    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "thinking_block: SessionResult flagged is_error");

    // Find the ThinkingDelta on the wire — content must match what the
    // scripted Reasoning block carried.
    let thinking_delta = harness.events.iter().find_map(|e| match e {
        CoreEvent::Stream(AgentStreamEvent::ThinkingDelta { delta, .. }) => Some(delta.as_str()),
        _ => None,
    });
    assert_eq!(
        thinking_delta,
        Some(thinking),
        "thinking_block: ThinkingDelta missing or wrong content (events={})",
        harness.events.len(),
    );

    // The final assistant text reached the chat.
    assert!(
        harness.assistant_text_contains(body),
        "thinking_block: assistant text body `{body}` missing — \
         got cells: {:?}",
        harness.text_cells_in_order(),
    );

    // After TurnCompleted, the streaming buffer is `take()`'n. Any leak
    // here would mean a stuck "still streaming" indicator on the TUI.
    assert!(
        harness.state.ui.streaming.is_none(),
        "thinking_block: state.ui.streaming should be None after \
         TurnCompleted, found Some",
    );

    harness.shutdown().await;
    Ok(())
}
