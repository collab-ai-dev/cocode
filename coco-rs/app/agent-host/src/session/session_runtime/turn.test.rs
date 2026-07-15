//! Deterministic regressions for the `SessionTurnCoordinator` turn-lifecycle
//! gate (CS-2 / R13).
//!
//! The end-to-end property — "a next turn sent immediately on terminal receipt
//! observes the previous turn's committed history" — is enforced by two
//! cooperating facts: the `SessionTurnExecutor` commits engine history *before*
//! releasing the held terminal event, and this coordinator keeps turn admission
//! CLOSED across the whole `Finishing` window so the next turn cannot start
//! until the lifecycle owner explicitly returns to `Idle`. The commit-ordering
//! half needs a full engine and is covered by the integration suite; the
//! admission-gating half is pure state-machine logic and is pinned here without
//! a model.

use super::*;

fn session_id() -> coco_types::SessionId {
    coco_types::SessionId::generate()
}

/// Inert active-turn handles for the coordinator's `start` closure. The tasks
/// never run meaningful work; these tests only assert lifecycle transitions.
fn dummy_handles(cancel: CancellationToken) -> ActiveTurnHandles {
    ActiveTurnHandles {
        cancel_token: cancel,
        turn_task: tokio::spawn(async {}),
        forwarder_task: tokio::spawn(async {}),
    }
}

#[tokio::test]
async fn start_is_rejected_while_a_turn_is_running() {
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("first turn admitted from Idle");
    assert!(
        coordinator
            .start(&sid, |_, cancel| dummy_handles(cancel))
            .is_err(),
        "a second turn must be rejected while the first is Running"
    );
}

#[tokio::test]
async fn start_stays_rejected_through_the_finishing_window() {
    // The terminal `TurnEnded` is delivered while the coordinator is Finishing
    // (history/accounting already committed, terminal in flight). Admission must
    // stay CLOSED for the entire Finishing window: a client that submits the
    // next turn the instant it observes the terminal cannot be admitted against
    // stale state, and only succeeds once the owner returns to Idle via
    // `complete_finishing`.
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("first turn admitted");
    assert!(coordinator.mark_finishing(), "Running -> Finishing");
    assert!(
        coordinator
            .start(&sid, |_, cancel| dummy_handles(cancel))
            .is_err(),
        "the next turn must NOT be admitted during the Finishing window"
    );
    assert!(coordinator.complete_finishing(), "Finishing -> Idle");
    coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("next turn admitted only after the owner returned to Idle");
}

#[tokio::test]
async fn complete_finishing_cannot_skip_the_finishing_state() {
    // Only the lifecycle owner drives Running -> Finishing -> Idle. Event
    // forwarding never calls `mark_finishing`, so it can neither push a Running
    // turn to Idle, clear its active handles, nor admit a new turn.
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("turn admitted");
    assert!(
        !coordinator.complete_finishing(),
        "complete_finishing from Running must be a no-op that stays Running"
    );
    assert!(
        coordinator
            .start(&sid, |_, cancel| dummy_handles(cancel))
            .is_err(),
        "the turn is still Running, so a new turn stays rejected"
    );
}

#[tokio::test]
async fn mark_finishing_from_idle_reports_no_active_turn() {
    let coordinator = SessionTurnCoordinator::default();
    assert!(
        !coordinator.mark_finishing(),
        "mark_finishing from Idle reports no active turn"
    );
    coordinator
        .start(&session_id(), |_, cancel| dummy_handles(cancel))
        .expect("an Idle coordinator admits a turn");
}

#[tokio::test]
async fn start_is_rejected_after_close_tombstone() {
    // A turn/start that resolved its target before the session close but runs
    // after it must be rejected: `close` tombstones the coordinator so no new
    // turn is admitted against a closed session.
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    coordinator.close();
    assert!(
        coordinator
            .start(&sid, |_, cancel| dummy_handles(cancel))
            .is_err(),
        "a turn must not be admitted after the coordinator is tombstoned"
    );
}

#[tokio::test]
async fn active_turn_id_tracks_the_running_turn() {
    // Server-request bridges read `active_turn_id()` to tag pending requests so
    // they can be cancelled at turn end. It must reflect the running/finishing
    // turn and be `None` when idle.
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    assert_eq!(
        coordinator.active_turn_id(),
        None,
        "idle has no active turn"
    );
    let turn_id = coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("turn admitted");
    assert_eq!(coordinator.active_turn_id(), Some(turn_id.clone()));
    assert!(coordinator.mark_finishing(), "Running -> Finishing");
    assert_eq!(
        coordinator.active_turn_id(),
        Some(turn_id),
        "the id survives the Finishing window"
    );
    assert!(coordinator.complete_finishing(), "Finishing -> Idle");
    assert_eq!(coordinator.active_turn_id(), None, "Idle again");
}

#[tokio::test]
async fn reserve_blocks_concurrent_start_and_releases_on_drop() {
    // A shortcut reservation holds the slot atomically: neither a real turn nor
    // a second shortcut can be admitted until it is released.
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    let (turn_id, cancel) = coordinator.reserve(&sid).expect("reservation admitted");
    assert_eq!(coordinator.active_turn_id(), Some(turn_id));
    assert!(!cancel.is_cancelled());
    assert!(
        coordinator
            .start(&sid, |_, cancel| dummy_handles(cancel))
            .is_err(),
        "a real turn must not be admitted while a shortcut holds the slot"
    );
    assert!(
        coordinator.reserve(&sid).is_err(),
        "a second shortcut must not be admitted"
    );
    coordinator.release_reservation();
    assert_eq!(coordinator.active_turn_id(), None);
    coordinator
        .start(&sid, |_, cancel| dummy_handles(cancel))
        .expect("a turn is admitted once the reservation is released");
}

#[tokio::test]
async fn close_cancels_a_reserved_shortcut() {
    let coordinator = SessionTurnCoordinator::default();
    let sid = session_id();
    let (_turn_id, cancel) = coordinator.reserve(&sid).expect("reservation admitted");
    coordinator
        .close()
        .expect("close returns the reserved shortcut's cancel token")
        .cancel();
    assert!(
        cancel.is_cancelled(),
        "close cancels a reserved shortcut so it cannot run against a closed session"
    );
}

#[tokio::test]
async fn close_cancels_a_running_turn_but_spares_a_finishing_one() {
    // `close` cancels a turn admitted in the drain->close race window (Running)
    // so it cannot run detached against a closed session, but leaves a Finishing
    // turn alone so its already-in-flight terminal is delivered rather than
    // superseded by a spurious cancel.
    let running = SessionTurnCoordinator::default();
    let sid = session_id();
    let mut running_token = None;
    running
        .start(&sid, |_, cancel| {
            running_token = Some(cancel.clone());
            dummy_handles(cancel)
        })
        .expect("running turn admitted");
    // `close` returns the Running turn's token for the caller to cancel (that is
    // what `SessionHandle::close_turn_coordinator` does).
    running
        .close()
        .expect("close returns a Running turn's cancel token")
        .cancel();
    assert!(
        running_token.expect("running token").is_cancelled(),
        "cancelling the token close returned must cancel the Running turn"
    );

    let finishing = SessionTurnCoordinator::default();
    let mut finishing_token = None;
    finishing
        .start(&sid, |_, cancel| {
            finishing_token = Some(cancel.clone());
            dummy_handles(cancel)
        })
        .expect("finishing turn admitted");
    assert!(finishing.mark_finishing(), "Running -> Finishing");
    assert!(
        finishing.close().is_none(),
        "close must not return (and the caller must not cancel) a Finishing turn"
    );
    assert!(
        !finishing_token.expect("finishing token").is_cancelled(),
        "close must not cancel a Finishing turn; its terminal is in flight"
    );
}
