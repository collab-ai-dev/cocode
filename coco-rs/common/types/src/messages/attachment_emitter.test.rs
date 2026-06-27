use super::*;
use crate::AttachmentKind;
use crate::HookCancelledPayload;
use crate::HookEventType;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

fn test_payload() -> AttachmentMessage {
    AttachmentMessage::silent_hook_cancelled(HookCancelledPayload {
        hook_name: "pre-commit".into(),
        tool_use_id: "tid".into(),
        hook_event: HookEventType::PreToolUse,
        command: None,
        duration_ms: None,
    })
}

#[test]
fn noop_drops_everything_and_reports_inactive() {
    let e = AttachmentEmitter::noop();
    assert!(!e.is_active());
    assert!(!e.emit(test_payload()));
}

#[tokio::test]
async fn live_emitter_forwards_to_receiver() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let e = AttachmentEmitter::new(tx);
    assert!(e.is_active());
    assert!(e.emit(test_payload()));
    let got = rx.recv().await.expect("receiver got the message");
    assert_eq!(got.kind, AttachmentKind::HookCancelled);
}

#[tokio::test]
async fn clone_shares_the_channel() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let e1 = AttachmentEmitter::new(tx);
    let e2 = e1.clone();
    assert!(e1.emit(test_payload()));
    assert!(e2.emit(test_payload()));
    assert_eq!(rx.recv().await.unwrap().kind, AttachmentKind::HookCancelled);
    assert_eq!(rx.recv().await.unwrap().kind, AttachmentKind::HookCancelled);
}

#[tokio::test]
async fn closed_channel_reports_inactive() {
    let (tx, rx) = mpsc::unbounded_channel();
    let e = AttachmentEmitter::new(tx);
    drop(rx);
    // Emit still tries and fails gracefully.
    assert!(!e.emit(test_payload()));
    assert!(!e.is_active());
}
