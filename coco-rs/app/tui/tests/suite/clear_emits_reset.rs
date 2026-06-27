//! Cross-layer regression guard for `/clear` (D1 fix).
//!
//! Pre-fix: `SessionRuntime::clear_conversation` cleared engine
//! `MessageHistory` but emitted no `ServerNotification` event. The
//! TUI's `TranscriptView` kept stale cells from the cleared session;
//! SDK NDJSON observers never saw the clear. Visible bug: after
//! `/clear`, the cleared transcript stayed on screen and the next
//! turn appeared interleaved with it.
//!
//! Post-fix: `/clear` emits `SessionResetForResume { session_id: new }`,
//! which drives the TUI teardown path
//! (`TranscriptView::on_session_reset`); SDK observers see it on the
//! wire.
//!
//! This test exercises the TUI-side reaction to that event — the
//! `tui_runner::run_clear_conversation` emit happens in the CLI
//! layer (not unit-testable from `app/tui`), but its product is
//! JSON-roundtripped here to prove it carries the right payload.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_messages::create_user_message;
use coco_tui::AppState;
use coco_tui::handle_event_for_test as handle_core_event;
use coco_tui::state::StreamingState;
use coco_tui::state::ToolExecution;
use coco_tui::state::ToolStatus;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use tokio::sync::mpsc;

fn protocol_evt(notif: ServerNotification) -> CoreEvent {
    CoreEvent::Protocol(notif)
}

/// JSON-roundtrip a `Protocol(ServerNotification)` event.
fn roundtrip(evt: CoreEvent) -> CoreEvent {
    let CoreEvent::Protocol(notif) = evt else {
        panic!("roundtrip helper only handles Protocol variants");
    };
    let json = serde_json::to_string(&notif).expect("ServerNotification serializes");
    let back: ServerNotification =
        serde_json::from_str(&json).expect("ServerNotification roundtrips through JSON");
    CoreEvent::Protocol(back)
}

fn fake_running_tool(call_id: &str) -> ToolExecution {
    ToolExecution {
        call_id: call_id.into(),
        name: "Read".into(),
        status: ToolStatus::Running,
        started_at: std::time::Instant::now(),
        completed_at: None,
        description: None,
        input_preview: None,
        streaming_input: None,
        message_uuid: Some(uuid::Uuid::nil()),
    }
}

/// `/clear` rotates session_id and emits `SessionResetForResume`. TUI
/// must wipe transcript + overlays and rotate `conversation_id`.
#[tokio::test]
async fn clear_full_emits_session_reset_and_wipes_state() {
    let mut state = AppState::new();

    // Seed prior-session state.
    for i in 0..3 {
        let m = create_user_message(&format!("prior {i}"));
        handle_core_event(
            &mut state,
            protocol_evt(ServerNotification::MessageAppended {
                message: std::sync::Arc::new(m),
                session_id: String::new(),
                agent_id: None,
            }),
        );
    }
    state.session.tool_executions.push(fake_running_tool("c1"));
    state.ui.streaming = Some(StreamingState::default());
    state.session.conversation_id = Some("pre-clear-session".into());
    assert_eq!(state.session.transcript.len(), 3, "prior cells seeded");

    // tui_runner emits this after runtime.clear_conversation rotates
    // the session id and clears the engine history.
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(4);
    tx.send(protocol_evt(ServerNotification::SessionResetForResume {
        session_id: "post-clear-session".into(),
        agent_id: None,
    }))
    .await
    .expect("channel accepts the event");

    let observed = rx.recv().await.expect("SDK observer receives event");
    let CoreEvent::Protocol(ServerNotification::SessionResetForResume { session_id, .. }) =
        &observed
    else {
        panic!("expected Protocol(SessionResetForResume), got {observed:?}");
    };
    assert_eq!(session_id, "post-clear-session");

    handle_core_event(&mut state, roundtrip(observed));

    assert!(
        state.session.transcript.is_empty(),
        "transcript wiped after SessionResetForResume"
    );
    assert!(
        state.session.tool_executions.is_empty(),
        "tool_executions wiped (no anchor survives a session reset)"
    );
    assert!(state.ui.streaming.is_none(), "streaming overlay wiped");
    assert_eq!(
        state.session.conversation_id.as_deref(),
        Some("post-clear-session"),
        "conversation_id rotates to the new session"
    );
}
