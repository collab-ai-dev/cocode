use crate::test_support::{
    evidence_record, evidence_ref, goal_id, queued_snapshot, satisfied_coverage, sid,
};
use crate::*;
use async_trait::async_trait;
use coco_goals::{
    BoundedText, CompletionPolicy, GoalSnapshot, GoalStatus, GoalTurnDisposition, ProgressSignal,
};
use pretty_assertions::assert_eq;
use std::sync::{Arc, Mutex};

/// A turn port that returns one canned outcome.
struct MockPort {
    outcome: Mutex<Option<GoalTurnOutcome>>,
}

impl MockPort {
    fn new(outcome: GoalTurnOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(Some(outcome)),
        })
    }
}

#[async_trait]
impl SessionTurnPort for MockPort {
    async fn start_goal_turn(&self, request: GoalTurnRequest) -> Result<GoalTurnHandle> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let outcome = self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("mock port outcome");
        let _ = tx.send(outcome);
        Ok(GoalTurnHandle {
            turn_id: request.turn_id,
            completion: GoalTurnCompletion::new(rx),
        })
    }
}

fn build(
    snapshot: GoalSnapshot,
    port: Arc<dyn SessionTurnPort>,
    evidence: Arc<dyn EvidenceStore>,
) -> (Arc<GoalRuntimeHandle>, GoalSupervisor) {
    let store = Arc::new(InMemoryGoalStore::new());
    let handle = Arc::new(GoalRuntimeHandle::new(sid(), store, Some(snapshot)));
    let materializer = Arc::new(GoalContextMaterializer::new(Arc::new(NoPlanSource)));
    let coordinator = Arc::new(GoalCompletionCoordinator::new(
        evidence,
        Arc::new(AlwaysVerified),
    ));
    let supervisor = GoalSupervisor::new(
        Arc::clone(&handle),
        port,
        materializer,
        coordinator,
        AutonomousAdmission::new(4),
    );
    (handle, supervisor)
}

fn progress_outcome() -> GoalTurnOutcome {
    GoalTurnOutcome::Ended {
        disposition: GoalTurnDisposition::Progress {
            summary: BoundedText::short("did work"),
            next_step: BoundedText::short("more"),
            evidence: Vec::new(),
        },
        signals: vec![ProgressSignal::ToolObservation],
        usage: coco_goals::UsageDelta::default(),
    }
}

#[tokio::test]
async fn test_advance_starts_and_continues_a_turn() {
    let (handle, supervisor) = build(
        queued_snapshot(CompletionPolicy::CandidateWithEvidence),
        MockPort::new(progress_outcome()),
        Arc::new(InMemoryEvidenceStore::new()),
    );
    let outcome = supervisor.advance().await.unwrap();
    assert_eq!(outcome, AdvanceOutcome::Advanced);
    // Progress → the goal is active again with a fresh queued lease.
    assert_eq!(handle.status().await, Some(GoalStatus::Active));
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot.counters.total_turns, 1);
    assert_eq!(snapshot.counters.autonomous_turns, 1);
}

#[tokio::test]
async fn test_advance_completes_on_verified_candidate() {
    let evidence = Arc::new(InMemoryEvidenceStore::new());
    evidence.record(evidence_record("e-1", &goal_id())).unwrap();
    let outcome = GoalTurnOutcome::Ended {
        disposition: GoalTurnDisposition::CompletionCandidate {
            coverage: satisfied_coverage(),
            evidence: vec![evidence_ref("e-1")],
        },
        signals: Vec::new(),
        usage: coco_goals::UsageDelta::default(),
    };
    let (handle, supervisor) = build(
        queued_snapshot(CompletionPolicy::CandidateWithEvidence),
        MockPort::new(outcome),
        evidence,
    );
    assert_eq!(
        supervisor.advance().await.unwrap(),
        AdvanceOutcome::Advanced
    );
    assert_eq!(handle.status().await, Some(GoalStatus::Completed));
}

#[tokio::test]
async fn test_advance_blocks_on_fatal_provider_error() {
    let (handle, supervisor) = build(
        queued_snapshot(CompletionPolicy::CandidateWithEvidence),
        MockPort::new(GoalTurnOutcome::ProviderError {
            kind: ProviderErrorKind::Fatal,
            message: "auth failed".to_string(),
        }),
        Arc::new(InMemoryEvidenceStore::new()),
    );
    assert_eq!(
        supervisor.advance().await.unwrap(),
        AdvanceOutcome::Advanced
    );
    assert_eq!(handle.status().await, Some(GoalStatus::Blocked));
}

#[tokio::test]
async fn test_advance_waits_on_retryable_provider_error() {
    let (handle, supervisor) = build(
        queued_snapshot(CompletionPolicy::CandidateWithEvidence),
        MockPort::new(GoalTurnOutcome::ProviderError {
            kind: ProviderErrorKind::Retryable,
            message: "429".to_string(),
        }),
        Arc::new(InMemoryEvidenceStore::new()),
    );
    assert_eq!(
        supervisor.advance().await.unwrap(),
        AdvanceOutcome::Advanced
    );
    assert_eq!(handle.status().await, Some(GoalStatus::Waiting));
}

#[tokio::test]
async fn test_advance_on_running_goal_is_not_startable() {
    // A goal already running a turn is not re-startable (idempotent reconcile).
    let mut snapshot =
        crate::test_support::running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    // Ensure it's running.
    let _ = &mut snapshot;
    let (_handle, supervisor) = build(
        snapshot,
        MockPort::new(progress_outcome()),
        Arc::new(InMemoryEvidenceStore::new()),
    );
    assert_eq!(
        supervisor.advance().await.unwrap(),
        AdvanceOutcome::NotStartable
    );
}

#[tokio::test]
async fn test_advance_budget_limited_when_autonomous_exhausted() {
    let mut snapshot = queued_snapshot(CompletionPolicy::CandidateWithEvidence);
    snapshot.counters.autonomous_turns = snapshot.budget.max_autonomous_turns.get();
    let (handle, supervisor) = build(
        snapshot,
        MockPort::new(progress_outcome()),
        Arc::new(InMemoryEvidenceStore::new()),
    );
    assert_eq!(
        supervisor.advance().await.unwrap(),
        AdvanceOutcome::BudgetLimited
    );
    assert_eq!(handle.status().await, Some(GoalStatus::BudgetLimited));
}
