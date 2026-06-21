use coco_types::SessionState;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::SessionStateTracker;
use crate::CoreEvent;
use crate::ServerNotification;

fn drain(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<SessionState> {
    let mut out = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let CoreEvent::Protocol(ServerNotification::SessionStateChanged { state }) = evt {
            out.push(state);
        }
    }
    out
}

#[tokio::test]
async fn emits_each_transition_once() {
    let tracker = SessionStateTracker::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    tracker.transition_to(SessionState::Running, &tx).await;
    tracker
        .transition_to(SessionState::RequiresAction, &tx)
        .await;
    tracker.transition_to(SessionState::Running, &tx).await;
    tracker.transition_to(SessionState::Idle, &tx).await;

    assert_eq!(
        drain(&mut rx),
        vec![
            SessionState::Running,
            SessionState::RequiresAction,
            SessionState::Running,
            SessionState::Idle,
        ],
    );
}

#[tokio::test]
async fn dedupes_consecutive_identical_states() {
    let tracker = SessionStateTracker::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    tracker.transition_to(SessionState::Running, &tx).await;
    tracker.transition_to(SessionState::Running, &tx).await;
    tracker.transition_to(SessionState::Running, &tx).await;

    assert_eq!(drain(&mut rx), vec![SessionState::Running]);
    assert_eq!(tracker.last(), Some(SessionState::Running));
}

#[tokio::test]
async fn no_op_when_sender_absent() {
    let tracker = SessionStateTracker::new();
    let tx: Option<mpsc::Sender<CoreEvent>> = None;

    tracker.transition_to(SessionState::Running, &tx).await;
    assert_eq!(tracker.last(), Some(SessionState::Running));

    tracker.transition_to(SessionState::Idle, &tx).await;
    assert_eq!(tracker.last(), Some(SessionState::Idle));
}

#[tokio::test]
async fn requires_action_round_trip_sequence() {
    // Models the approval flow: Running → RequiresAction → Running → Idle.
    let tracker = SessionStateTracker::new();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let tx = Some(tx);

    tracker.transition_to(SessionState::Running, &tx).await;
    tracker
        .transition_to(SessionState::RequiresAction, &tx)
        .await;
    // The engine may re-emit Running on multiple exit branches; dedup collapses.
    tracker.transition_to(SessionState::Running, &tx).await;
    tracker.transition_to(SessionState::Running, &tx).await;
    tracker.transition_to(SessionState::Idle, &tx).await;

    assert_eq!(
        drain(&mut rx),
        vec![
            SessionState::Running,
            SessionState::RequiresAction,
            SessionState::Running,
            SessionState::Idle,
        ],
    );
}
