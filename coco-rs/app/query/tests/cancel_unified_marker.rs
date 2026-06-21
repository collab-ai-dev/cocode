//! Cross-layer regression guard for `finalize_user_cancel` (plan §5.3, §9).
//!
//! Verifies the single cancel-finalizer:
//!   1. Pushes one `Message::System(SystemMessage::UserInterruption)`.
//!   2. Stores `for_tool_use` on the typed variant so TUI / SDK never
//!      recompute it.
//!   3. Emits exactly one `MessageAppended` whose payload mirrors the
//!      history entry.
//!   4. Is idempotent — a second invocation against an already-marked
//!      history is a no-op (dedup predicate).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::SystemMessage;
use coco_query::history_sync;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use tokio::sync::mpsc;

#[tokio::test]
async fn cancel_no_tools_marks_for_tool_use_false_and_emits_one_event() {
    let mut history = MessageHistory::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);

    history_sync::finalize_user_cancel(&mut history, false, &Some(tx.clone())).await;

    assert_eq!(history.len(), 1, "exactly one message pushed");
    let last = history.as_slice().last().expect("history non-empty");
    let Message::System(SystemMessage::UserInterruption(m)) = last.as_ref() else {
        panic!("expected SystemMessage::UserInterruption, got {last:?}");
    };
    assert!(
        !m.for_tool_use,
        "no tool was running, for_tool_use must be false"
    );

    let event = rx.try_recv().expect("MessageAppended emitted");
    let CoreEvent::Protocol(ServerNotification::MessageAppended { message, .. }) = event else {
        panic!("expected CoreEvent::Protocol(MessageAppended)");
    };
    let Message::System(SystemMessage::UserInterruption(emitted)) = message.as_ref() else {
        panic!("emitted payload wrong variant");
    };
    assert!(
        !emitted.for_tool_use,
        "wire payload must mirror history's for_tool_use"
    );

    assert!(
        rx.try_recv().is_err(),
        "single push must emit exactly one event"
    );
}

#[tokio::test]
async fn cancel_during_tool_marks_for_tool_use_true() {
    let mut history = MessageHistory::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);

    history_sync::finalize_user_cancel(&mut history, true, &Some(tx.clone())).await;

    let last = history.as_slice().last().expect("history non-empty");
    let Message::System(SystemMessage::UserInterruption(m)) = last.as_ref() else {
        panic!("expected SystemMessage::UserInterruption");
    };
    assert!(
        m.for_tool_use,
        "tool was running, for_tool_use must be true"
    );

    let event = rx.try_recv().expect("event emitted");
    let CoreEvent::Protocol(ServerNotification::MessageAppended { message, .. }) = event else {
        panic!("wrong event variant");
    };
    let Message::System(SystemMessage::UserInterruption(emitted)) = message.as_ref() else {
        panic!("wrong payload variant");
    };
    assert!(emitted.for_tool_use);
}

#[tokio::test]
async fn cancel_is_idempotent_against_existing_marker() {
    let mut history = MessageHistory::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);

    history_sync::finalize_user_cancel(&mut history, false, &Some(tx.clone())).await;
    history_sync::finalize_user_cancel(&mut history, true, &Some(tx.clone())).await;

    assert_eq!(
        history.len(),
        1,
        "second cancel must dedup, history stays at 1 entry"
    );

    let _first = rx.try_recv().expect("first cancel emitted");
    assert!(
        rx.try_recv().is_err(),
        "deduped second cancel must not emit a second event"
    );
}
