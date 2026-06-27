use super::finalize_user_cancel;
use super::history_push_and_emit;
use super::is_steering_interrupt;
use super::last_message_is_user_interruption;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::SystemMessage;
use coco_messages::TombstoneMessage;
use coco_messages::create_progress_message;
use coco_messages::create_user_interruption_system_message;
use coco_messages::create_user_message;
use coco_types::CoreEvent;
use coco_types::MessageKind;
use coco_types::ServerNotification;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use uuid::Uuid;

#[test]
fn is_steering_interrupt_only_for_submit_interrupt() {
    use coco_types::TurnAbortReason;
    // Submit-interrupt (typed/queued input while tools ran) is steering — the
    // queued message provides continuity, so the standalone marker is skipped.
    assert!(is_steering_interrupt(Some(
        TurnAbortReason::SubmitInterrupt
    )));
    // Every other reason is a hard cancel / preempt — keep the marker.
    assert!(!is_steering_interrupt(Some(TurnAbortReason::UserCancel)));
    assert!(!is_steering_interrupt(Some(TurnAbortReason::SystemPreempt)));
    assert!(!is_steering_interrupt(Some(
        TurnAbortReason::PermissionAbort
    )));
    assert!(!is_steering_interrupt(Some(TurnAbortReason::Background)));
    // No reason recorded (e.g. bare-token cancel paths) → keep the marker.
    assert!(!is_steering_interrupt(None));
}

#[tokio::test]
async fn finalize_user_cancel_pushes_typed_marker_and_emits() {
    let mut history = MessageHistory::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    finalize_user_cancel(&mut history, true, &tx).await;

    assert_eq!(history.len(), 1);
    let Message::System(SystemMessage::UserInterruption(m)) =
        history.first().expect("non-empty").as_ref()
    else {
        panic!("expected typed UserInterruption variant");
    };
    assert!(m.for_tool_use);

    let evt = rx.try_recv().expect("MessageAppended emitted");
    let CoreEvent::Protocol(ServerNotification::MessageAppended { .. }) = evt else {
        panic!("expected MessageAppended protocol event");
    };
}

#[tokio::test]
async fn finalize_user_cancel_dedups_against_typed_marker() {
    let mut history = MessageHistory::new();
    let (tx, _rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    finalize_user_cancel(&mut history, false, &tx).await;
    finalize_user_cancel(&mut history, false, &tx).await;

    assert_eq!(history.len(), 1);
}

#[test]
fn last_message_is_user_interruption_recognizes_typed_form() {
    let mut history = MessageHistory::new();
    assert!(!last_message_is_user_interruption(&history));
    history.push(create_user_interruption_system_message(false));
    assert!(last_message_is_user_interruption(&history));
}

#[test]
fn last_message_is_user_interruption_recognizes_legacy_text_form() {
    let mut history = MessageHistory::new();
    history.push(coco_messages::create_user_interruption_message(true));
    assert!(last_message_is_user_interruption(&history));
}

#[test]
fn last_message_is_user_interruption_negative() {
    let mut history = MessageHistory::new();
    history.push(create_user_message("hi"));
    assert!(!last_message_is_user_interruption(&history));
}

#[tokio::test]
async fn history_push_and_emit_emits_when_channel_attached() {
    let mut history = MessageHistory::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);
    history_push_and_emit(&mut history, create_user_message("hi"), &tx).await;
    assert_eq!(history.len(), 1);
    let evt = rx.try_recv().expect("expected event");
    let CoreEvent::Protocol(ServerNotification::MessageAppended { .. }) = evt else {
        panic!("expected MessageAppended");
    };
}

#[tokio::test]
async fn history_push_and_emit_is_no_op_event_when_channel_none() {
    let mut history = MessageHistory::new();
    let tx: Option<mpsc::Sender<CoreEvent>> = None;
    history_push_and_emit(&mut history, create_user_message("hi"), &tx).await;
    assert_eq!(history.len(), 1);
}

/// Regression: a `Progress` row arriving between two rapid cancel
/// signals must not unmask the prior `UserInterruption` for dedup.
/// Without the tail-scan fix, the second cancel sees the Progress as
/// `last()`, returns `false`, and pushes a duplicate marker.
#[tokio::test]
async fn finalize_user_cancel_dedups_through_progress_interleave() {
    let mut history = MessageHistory::new();
    let (tx, _rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    finalize_user_cancel(&mut history, true, &tx).await;
    // Late progress for the in-flight tool (UI-only, doesn't reach API).
    history.push(create_progress_message(
        "tool-1",
        serde_json::json!({"pct": 50}),
    ));
    finalize_user_cancel(&mut history, true, &tx).await;

    // Exactly one UserInterruption marker, plus the Progress row.
    let interruptions = history
        .as_slice()
        .iter()
        .filter(|m| {
            matches!(
                (**m).as_ref(),
                Message::System(SystemMessage::UserInterruption(_))
            )
        })
        .count();
    assert_eq!(interruptions, 1, "duplicate UserInterruption past Progress");
}

/// Tombstones are UI-only ephemera too; tail-scan must skip them
/// when classifying the trailing semantic message.
#[tokio::test]
async fn finalize_user_cancel_dedups_through_tombstone_interleave() {
    let mut history = MessageHistory::new();
    let (tx, _rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    finalize_user_cancel(&mut history, false, &tx).await;
    history.push(Message::Tombstone(TombstoneMessage {
        uuid: Uuid::new_v4(),
        original_kind: MessageKind::Progress,
    }));
    finalize_user_cancel(&mut history, false, &tx).await;

    let interruptions = history
        .as_slice()
        .iter()
        .filter(|m| {
            matches!(
                (**m).as_ref(),
                Message::System(SystemMessage::UserInterruption(_))
            )
        })
        .count();
    assert_eq!(
        interruptions, 1,
        "duplicate UserInterruption past Tombstone"
    );
}
