//! Cross-layer regression guard for `SessionResetForResume` + bulk
//! `MessageAppended` replay (plan §6.3, §9).
//!
//! Simulates the resume burst: the engine emits
//! `SessionResetForResume` and then replays every loaded history
//! entry through `MessageAppended`. Verifies:
//!
//!   1. Reset clears `transcript`, `tool_executions`, and `ui.streaming`
//!      so prior-session overlays don't leak into the resumed view.
//!   2. `conversation_id` rotates to the new session id (prompt-cache
//!      collision guard).
//!   3. Each replayed `MessageAppended` re-derives a cell from the
//!      typed wire payload — i.e. the `Message` enum survives the
//!      JSON roundtrip with no `serde_json::Value` erasure.
//!   4. Typed `SystemMessage::UserInterruption` carries `for_tool_use`
//!      across the wire untouched.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_messages::Message;
use coco_messages::SystemMessage;
use coco_messages::create_user_interruption_system_message;
use coco_messages::create_user_message;
use coco_tui::AppState;
use coco_tui::handle_event_for_test as handle_core_event;
use coco_tui::state::StreamingState;
use coco_tui::state::ToolExecution;
use coco_tui::state::ToolStatus;
use coco_types::CoreEvent;
use coco_types::ServerNotification;

/// JSON-roundtrip the wire payload, then wrap in `CoreEvent::Protocol`.
/// `CoreEvent` itself is in-process only (no serde derives); the
/// wire-visible type is `ServerNotification` and that's what the SDK
/// emits to NDJSON. Roundtripping here proves
/// `MessageAppended.message` carries a typed `Message` end-to-end —
/// if it ever regresses to `serde_json::Value`, deser would lose the
/// enum discriminant.
fn roundtrip_notif(notif: ServerNotification) -> ServerNotification {
    let json = serde_json::to_string(&notif).expect("ServerNotification serializes");
    serde_json::from_str(&json).expect("ServerNotification roundtrips through JSON")
}

fn protocol_evt(notif: ServerNotification) -> CoreEvent {
    CoreEvent::Protocol(roundtrip_notif(notif))
}

fn fake_running_tool() -> ToolExecution {
    ToolExecution {
        call_id: "stale-call".into(),
        name: "Read".into(),
        status: ToolStatus::Running,
        started_at: std::time::Instant::now(),
        completed_at: None,
        description: None,
        input_preview: None,
        streaming_input: None,
        // Stamped to an arbitrary UUID — SessionResetForResume wipes
        // every execution regardless of anchor, so the value is
        // immaterial for this test.
        message_uuid: Some(uuid::Uuid::nil()),
    }
}

#[test]
fn resume_clears_prior_overlays_then_rebuilds_transcript() {
    let mut state = AppState::new();

    // ── Seed prior-session state ────────────────────────────────────
    let stale_msg = create_user_message("stale prior-session prompt");
    handle_core_event(
        &mut state,
        protocol_evt(ServerNotification::MessageAppended {
            message: std::sync::Arc::new(stale_msg),
            session_id: String::new(),
            agent_id: None,
        }),
    );
    state.session.tool_executions.push(fake_running_tool());
    state.ui.streaming = Some(StreamingState::default());
    state.session.conversation_id = Some("prior-session".into());

    assert_eq!(state.session.transcript.len(), 1, "stale cell seeded");
    assert!(!state.session.tool_executions.is_empty());
    assert!(state.ui.streaming.is_some());

    // ── Emit SessionResetForResume ──────────────────────────────────
    handle_core_event(
        &mut state,
        protocol_evt(ServerNotification::SessionResetForResume {
            session_id: "resumed-001".into(),
            agent_id: None,
        }),
    );

    assert!(
        state.session.transcript.is_empty(),
        "reset wipes derived transcript"
    );
    assert!(
        state.session.tool_executions.is_empty(),
        "reset wipes tool overlays"
    );
    assert!(
        state.ui.streaming.is_none(),
        "reset wipes streaming overlay"
    );
    assert_eq!(
        state.session.conversation_id.as_deref(),
        Some("resumed-001"),
        "conversation_id rotates to the new session"
    );

    // ── Replay history through MessageAppended ──────────────────────
    let m1 = create_user_message("first resumed prompt");
    let m2 = create_user_interruption_system_message(true);
    let m2_uuid = *m2.uuid().expect("system interruption carries uuid");

    handle_core_event(
        &mut state,
        protocol_evt(ServerNotification::MessageAppended {
            message: std::sync::Arc::new(m1),
            session_id: String::new(),
            agent_id: None,
        }),
    );
    let m2_event = roundtrip_notif(ServerNotification::MessageAppended {
        message: std::sync::Arc::new(m2),
        session_id: String::new(),
        agent_id: None,
    });
    let ServerNotification::MessageAppended { message, .. } = &m2_event else {
        panic!("expected MessageAppended after roundtrip");
    };
    let Message::System(SystemMessage::UserInterruption(interruption)) = message.as_ref() else {
        panic!("expected typed UserInterruption after JSON roundtrip");
    };
    assert!(
        interruption.for_tool_use,
        "for_tool_use must survive the JSON roundtrip untouched"
    );
    handle_core_event(&mut state, CoreEvent::Protocol(m2_event));

    assert_eq!(
        state.session.transcript.len(),
        2,
        "two replayed messages -> two cells"
    );
    assert!(!m2_uuid.is_nil(), "interruption carries uuid");
}

#[test]
fn resume_with_no_prior_state_still_sets_conversation_id() {
    let mut state = AppState::new();
    assert!(state.session.transcript.is_empty());

    handle_core_event(
        &mut state,
        protocol_evt(ServerNotification::SessionResetForResume {
            session_id: "fresh-resume".into(),
            agent_id: None,
        }),
    );

    assert!(state.session.transcript.is_empty());
    assert_eq!(
        state.session.conversation_id.as_deref(),
        Some("fresh-resume"),
        "id rotation fires even when there is nothing to clear"
    );
}
