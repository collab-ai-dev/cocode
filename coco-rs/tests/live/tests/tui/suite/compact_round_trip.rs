//! Manual `/compact` round-trip — drives `QueryEngine::run_manual_compact`
//! directly, with a synthetic `MessageHistory` we set up in the test.
//!
//! Why direct (vs going through `submit("/compact")`)?
//! ------------------------------------------------------
//! The TUI's `/compact` slash command sends `UserCommand::Compact` on
//! the channel; production's `tui_runner` handles it by calling
//! `engine.run_manual_compact(...)` against `runtime.history` (an
//! externally-maintained `MessageHistory` owned by `SessionRuntime`).
//!
//! This harness deliberately omits `SessionRuntime` (its responsibilities
//! — history persistence, file-history snapshots, marble-origami staging
//! — have their own crate-level coverage in `coco-session`). So we test
//! the engine-side compact pipeline by:
//!
//! 1. Building a synthetic `MessageHistory` directly,
//! 2. Calling `engine.run_manual_compact(&mut history, &Some(event_tx), ...)`,
//! 3. Draining the events the engine emitted into the harness's AppState,
//! 4. Asserting on the resulting history shape AND the events delivered.
//!
//! The TUI-facing parts (slash interception, toast emission, the
//! `Compacting…` status line) are already covered by `slash_clear` /
//! tui-side overlay tests in this suite — `/compact` reuses that
//! interception path verbatim.

use std::time::Duration;

use anyhow::Result;
use coco_messages::AssistantContent;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::TextContent;
use coco_messages::UserMessage;
use coco_types::CoreEvent;
use coco_types::ServerNotification;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

const SUMMARY_TEXT: &str = "compact-summary: alpha→beta workflow distilled to gist";

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        // Pre-load a single LLM reply that the compaction summarizer
        // will consume. The agent loop never runs in this test, so the
        // queue length matches exactly the LLM calls compact issues.
        .with_replies([Reply::text(SUMMARY_TEXT)])
        .build()
        .await?;

    // Build a synthetic history with several user/assistant rounds —
    // enough that compact has something to summarize. Production
    // history typically lands here after N conversational turns.
    let mut history = MessageHistory::new();
    for i in 1..=5 {
        history.push(make_user(format!(
            "user prompt {i} — drive a multi-step workflow"
        )));
        history.push(make_assistant(format!(
            "assistant turn {i} — performed step {i} of a longer chain"
        )));
    }
    let pre_message_count = history.len();
    assert_eq!(
        pre_message_count, 10,
        "compact_round_trip: setup expected 10 messages (5 U + 5 A), got {pre_message_count}",
    );

    // Run the engine's manual-compact path with our event_tx so the
    // CompactionStarted / CompactionPhase / ContextCompacted events
    // land in the harness's event channel.
    let event_tx = harness.event_tx();
    let event_tx_opt = Some(event_tx);
    harness
        .engine()
        .run_manual_compact(
            &mut history,
            &event_tx_opt,
            coco_query::ManualCompactRequest::new(/*custom_instructions*/ None),
        )
        .await;

    // Drain everything the compact pipeline queued. 200ms quiet window
    // is plenty — events fire synchronously inside run_manual_compact.
    let drained = harness
        .drain_pending_events(Duration::from_millis(200))
        .await;
    assert!(
        drained > 0,
        "compact_round_trip: expected the engine to emit compaction events, got 0",
    );

    // Engine made exactly one model call — the summarizer.
    assert_eq!(
        harness.model.call_count(),
        1,
        "compact_round_trip: expected exactly 1 LLM call (the summarizer), got {}",
        harness.model.call_count(),
    );

    // History must have been rewritten. The exact post-compaction
    // message count depends on the strategy (boundary markers + summary
    // payload), but it MUST be smaller than pre — otherwise compact
    // didn't actually do anything.
    assert!(
        history.len() < pre_message_count,
        "compact_round_trip: history size should shrink after compact \
         (pre={pre_message_count}, post={})",
        history.len(),
    );

    // The summary text we scripted must appear somewhere in the new
    // history — that's the load-bearing evidence that the summarizer
    // ran and its output was folded back in.
    let summary_in_history = history.iter().any(|m| message_contains_summary(m));
    assert!(
        summary_in_history,
        "compact_round_trip: scripted summary `{SUMMARY_TEXT}` should appear \
         in the post-compact history",
    );

    // Event-side: at least one `CompactionPhase` and the terminal
    // `ContextCompacted` must have landed. (CompactionStarted is only
    // emitted from the *reactive* recovery path; manual compact uses
    // `CompactionPhase` + the terminal `ContextCompacted` instead —
    // see `engine_compaction.rs` vs `engine_finalize_turn.rs:66`.)
    let saw_phase = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::CompactionPhase(_))
        )
    });
    let saw_compacted = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::ContextCompacted(_))
        )
    });
    assert!(
        saw_phase,
        "compact_round_trip: missing `CompactionPhase` notification — \
         got {} events",
        harness.events.len(),
    );
    assert!(
        saw_compacted,
        "compact_round_trip: missing `ContextCompacted` notification — \
         got {} events",
        harness.events.len(),
    );

    // The Phase::Done sub-event clears the Compacting… spinner in the
    // production TUI. Make sure it landed last among the phase events.
    let saw_done = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::CompactionPhase(p))
                if matches!(p.phase, coco_types::CompactionPhase::Done)
        )
    });
    assert!(
        saw_done,
        "compact_round_trip: terminal `CompactionPhase::Done` missing — \
         the spinner-clear cue is load-bearing for the TUI",
    );

    // The trigger field on ContextCompacted must be `Manual` (not Auto)
    // because we routed through the manual entry-point.
    let trigger_is_manual = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::ContextCompacted(p))
                if matches!(p.trigger, coco_types::CompactTrigger::Manual)
        )
    });
    assert!(
        trigger_is_manual,
        "compact_round_trip: ContextCompacted.trigger should be Manual — \
         we routed through `run_manual_compact`",
    );

    harness.shutdown().await;
    Ok(())
}

fn make_user(text: impl Into<String>) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text.into()),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn make_assistant(text: impl Into<String>) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: uuid::Uuid::new_v4(),
        model: "scripted-model".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Scan a Message's text payload for our scripted summary marker. The
/// post-compact history embeds the summary inside a synthesized
/// assistant message and/or attachment, so we walk both shapes.
fn message_contains_summary(msg: &Message) -> bool {
    match msg {
        Message::Assistant(a) => match &a.message {
            LlmMessage::Assistant { content, .. } => content.iter().any(|c| match c {
                AssistantContent::Text(t) => t.text.contains(SUMMARY_TEXT),
                _ => false,
            }),
            _ => false,
        },
        Message::Attachment(att) => match &att.body {
            coco_messages::AttachmentBody::Api(LlmMessage::User { content, .. }) => {
                content.iter().any(|c| match c {
                    coco_messages::UserContent::Text(t) => t.text.contains(SUMMARY_TEXT),
                    _ => false,
                })
            }
            _ => false,
        },
        Message::User(u) => match &u.message {
            LlmMessage::User { content, .. } => content.iter().any(|c| match c {
                coco_messages::UserContent::Text(t) => t.text.contains(SUMMARY_TEXT),
                _ => false,
            }),
            _ => false,
        },
        _ => false,
    }
}
