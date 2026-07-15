use super::*;
use coco_types::TurnId;
use pretty_assertions::assert_eq;

fn queued() -> GoalLease {
    GoalLease::Queued {
        lease_id: GoalLeaseId::new("l-1"),
        attempt: 0,
    }
}

fn running() -> GoalLease {
    GoalLease::Running {
        lease_id: GoalLeaseId::new("l-1"),
        turn_id: TurnId::new("t-1"),
    }
}

#[test]
fn test_lease_accessors() {
    assert!(queued().is_queued());
    assert!(!queued().is_running());
    assert_eq!(queued().running_turn(), None);
    assert!(running().is_running());
    assert_eq!(running().running_turn(), Some(&TurnId::new("t-1")));
    assert_eq!(running().lease_id(), &GoalLeaseId::new("l-1"));
}

#[test]
fn test_lifecycle_status_projection() {
    assert_eq!(
        GoalLifecycle::Active { lease: queued() }.status(),
        GoalStatus::Active
    );
    assert_eq!(
        GoalLifecycle::Paused {
            reason: PauseReason::NoProgress
        }
        .status(),
        GoalStatus::Paused
    );
}

#[test]
fn test_active_always_exposes_lease_and_running_lease() {
    let active = GoalLifecycle::Active { lease: running() };
    assert!(active.lease().is_some());
    assert_eq!(active.running_lease_id(), Some(&GoalLeaseId::new("l-1")));
    assert!(active.has_automatic_work());

    let queued_active = GoalLifecycle::Active { lease: queued() };
    assert_eq!(queued_active.running_lease_id(), None);
}

#[test]
fn test_waiting_always_exposes_wake() {
    let wake = GoalWake {
        wake_id: WakeId::new("w-1"),
        condition: WaitCondition::Deadline {
            deadline: Timestamp::from_millis(10),
        },
    };
    let waiting = GoalLifecycle::Waiting { wake: wake.clone() };
    assert_eq!(waiting.wake(), Some(&wake));
    assert!(!waiting.has_automatic_work());
}

#[test]
fn test_status_stopped_and_terminal_classification() {
    assert!(GoalStatus::Paused.is_stopped());
    assert!(GoalStatus::Blocked.is_stopped());
    assert!(GoalStatus::UsageLimited.is_stopped());
    assert!(GoalStatus::BudgetLimited.is_stopped());
    assert!(!GoalStatus::Active.is_stopped());
    assert!(!GoalStatus::Waiting.is_stopped());
    assert!(!GoalStatus::Completed.is_stopped());
    assert!(GoalStatus::Completed.is_terminal());
}

#[test]
fn test_lifecycle_tagged_serde_roundtrip() {
    let lifecycle = GoalLifecycle::Active { lease: running() };
    let json = serde_json::to_value(&lifecycle).unwrap();
    assert_eq!(json["status"], "active");
    assert_eq!(json["lease"]["kind"], "running");
    assert_eq!(json["lease"]["lease_id"], "l-1");
    let back: GoalLifecycle = serde_json::from_value(json).unwrap();
    assert_eq!(back, lifecycle);
}
