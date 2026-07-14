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
