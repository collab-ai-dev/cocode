//! Tests for the emit helpers. These lock two invariants:
//!
//! 1. A `None` sender (headless mode) is a no-op that reports success.
//!    Without this, callers that conditionally attach events to a query
//!    run would have to re-check the Option at every call site.
//! 2. Events are routed to the correct `CoreEvent` layer. Previously we
//!    had `let _ = tx.send(CoreEvent::Protocol(...)).await` at every call
//!    site — a typo could route a Stream event through Protocol without
//!    any test catching it.

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TuiOnlyEvent;
use coco_types::TurnStartedParams;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::emit;
use super::emit_protocol;
use super::emit_protocol_owned;
use super::emit_stream;
use super::emit_tui;

fn make_protocol_event() -> ServerNotification {
    ServerNotification::TurnStarted(TurnStartedParams {
        turn_id: coco_types::TurnId::from("t1"),
    })
}

#[tokio::test]
async fn emit_protocol_none_sender_succeeds_without_panic() {
    // Headless mode: no consumer attached. Emission must not panic and
    // must report success so callers don't mis-interpret the result as
    // "channel closed, stop running".
    let tx: Option<mpsc::Sender<CoreEvent>> = None;
    let ok = emit_protocol(&tx, make_protocol_event()).await;
    assert!(ok, "None sender must be treated as headless success");
}

#[tokio::test]
async fn emit_protocol_closed_channel_reports_false() {
    let (tx, rx) = mpsc::channel::<CoreEvent>(1);
    drop(rx); // simulate consumer dropping
    let ok = emit_protocol(&Some(tx), make_protocol_event()).await;
    assert!(!ok, "closed channel must report false so callers can react");
}

#[tokio::test]
async fn emit_protocol_routes_to_protocol_layer() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(1);
    assert!(emit_protocol(&Some(tx), make_protocol_event()).await);

    let received = rx.recv().await.expect("event delivered");
    match received {
        CoreEvent::Protocol(ServerNotification::TurnStarted(p)) => {
            assert_eq!(p.turn_id.as_str(), "t1");
        }
        other => panic!("expected Protocol(TurnStarted), got {other:?}"),
    }
}

#[tokio::test]
async fn emit_stream_routes_to_stream_layer() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(1);
    let evt = AgentStreamEvent::TextDelta {
        turn_id: "t1".into(),
        delta: "hello".into(),
    };
    assert!(emit_stream(&Some(tx), evt).await);

    match rx.recv().await.expect("event delivered") {
        CoreEvent::Stream(AgentStreamEvent::TextDelta { delta, .. }) => {
            assert_eq!(delta, "hello");
        }
        other => panic!("expected Stream(TextDelta), got {other:?}"),
    }
}

#[tokio::test]
async fn emit_tui_routes_to_tui_layer() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(1);
    let evt = TuiOnlyEvent::ApprovalRequired {
        request_id: "r1".into(),
        tool_name: "Bash".into(),
        description: "rm -rf".into(),
        display_input: coco_types::PermissionDisplayInput::Command("rm -rf".into()),
        show_always_allow: true,
        choices: None,
        detail: None,
        permission_suggestions: vec![],
        original_input: None,
        cwd: None,
        worker_badge: None,
    };
    assert!(emit_tui(&Some(tx), evt).await);

    match rx.recv().await.expect("event delivered") {
        CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired { request_id, .. }) => {
            assert_eq!(request_id, "r1");
        }
        other => panic!("expected Tui(ApprovalRequired), got {other:?}"),
    }
}

#[tokio::test]
async fn emit_generic_delegates() {
    // The generic `emit` must deliver whatever CoreEvent variant it's given
    // without inspection — it's the escape hatch for uncommon paths that
    // construct the envelope themselves.
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(1);
    assert!(emit(&Some(tx), CoreEvent::Protocol(make_protocol_event())).await);
    assert!(matches!(
        rx.recv().await,
        Some(CoreEvent::Protocol(ServerNotification::TurnStarted(_)))
    ));
}

#[tokio::test]
async fn emit_protocol_owned_reports_channel_closed() {
    let (tx, rx) = mpsc::channel::<CoreEvent>(1);
    drop(rx);
    let ok = emit_protocol_owned(&tx, make_protocol_event()).await;
    assert!(!ok);
}
