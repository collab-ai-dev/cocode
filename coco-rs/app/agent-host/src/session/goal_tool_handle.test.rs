use super::*;
use coco_goal_runtime::InMemoryGoalStore;
use coco_goals::{
    BoundedText, EvidenceId, EvidenceRef, GoalStatus, RequirementCoverage, RequirementResult,
};
use coco_tool_runtime::{GoalContinuation, GoalCreateRequest, GoalHandle, ToolEvidenceObservation};
use pretty_assertions::assert_eq;

fn cite(id: &str) -> EvidenceRef {
    EvidenceRef {
        evidence_id: EvidenceId::new(id),
        summary: BoundedText::short("cited"),
    }
}

fn satisfied_coverage() -> RequirementCoverage {
    RequirementCoverage {
        requirements: vec![RequirementResult {
            requirement: BoundedText::short("feature shipped"),
            satisfied: true,
            evidence: Vec::new(),
        }],
        asserts_complete: true,
    }
}

fn handle() -> SessionGoalHandle {
    let store = Arc::new(InMemoryGoalStore::new());
    let sid = match coco_types::SessionId::try_new("goal-turn-test") {
        Ok(id) => id,
        Err(_) => unreachable!("valid session id"),
    };
    SessionGoalHandle::new(
        Arc::new(GoalRuntimeHandle::new(sid, store, None)),
        Arc::new(coco_goal_runtime::NoPlanSource),
        Arc::new(coco_goal_runtime::InMemoryEvidenceStore::new()),
        Arc::new(tokio::sync::Notify::new()),
    )
}

async fn create(handle: &SessionGoalHandle) {
    handle
        .create_goal(GoalCreateRequest {
            objective: "ship the feature".to_string(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn test_bind_without_goal_is_false() {
    let handle = handle();
    assert!(!handle.bind_turn("t0".to_string()).await);
}

#[tokio::test]
async fn test_progress_turn_finalizes_to_stop_leaving_goal_queued() {
    let handle = handle();
    create(&handle).await;
    assert!(handle.bind_turn("t0".to_string()).await);
    handle
        .report_turn(GoalTurnDisposition::Progress {
            summary: BoundedText::short("did work"),
            next_step: BoundedText::short("more work"),
            evidence: Vec::new(),
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(10, 5, true).await;
    // §10.3: the engine runs one logical turn and never self-continues; the driver
    // owns the next turn. Finalize returns Stop with the goal left active+queued.
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    // Only the one finished turn — no second turn was self-started.
    assert_eq!(snapshot.counters.total_turns, 1);
}

#[tokio::test]
async fn test_completion_candidate_completes_and_stops() {
    let handle = handle();
    create(&handle).await;
    handle.bind_turn("t0".to_string()).await;
    handle
        .report_turn(GoalTurnDisposition::CompletionCandidate {
            coverage: RequirementCoverage {
                requirements: vec![RequirementResult {
                    requirement: BoundedText::short("feature shipped"),
                    satisfied: true,
                    evidence: Vec::new(),
                }],
                asserts_complete: true,
            },
            evidence: Vec::new(),
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(1, 1, true).await;
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    // The completed transition emits a met transcript cell (§9.2).
    let cell = continuation
        .transition
        .expect("completion emits a transition cell");
    assert!(cell.met);
    assert!(!cell.failed);
    assert_eq!(
        handle.snapshot().await.unwrap().status(),
        GoalStatus::Completed
    );
}

#[tokio::test]
async fn test_progress_turn_has_no_transition_cell() {
    let handle = handle();
    create(&handle).await;
    handle.bind_turn("t0".to_string()).await;
    handle
        .report_turn(GoalTurnDisposition::Progress {
            summary: BoundedText::short("did work"),
            next_step: BoundedText::short("more work"),
            evidence: Vec::new(),
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(10, 5, true).await;
    // An ongoing goal is continuing, not transitioning — no cell. Finalize always
    // returns Stop now (§10.3); the driver owns the next turn.
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    assert!(continuation.transition.is_none());
}

#[tokio::test]
async fn test_cited_minted_evidence_completes_the_goal() {
    let handle = handle();
    create(&handle).await;
    handle.bind_turn("t0".to_string()).await;
    // The runtime mints provenance for an accepted tool result this turn.
    handle
        .record_tool_evidence(ToolEvidenceObservation {
            tool_use_id: "call-1".to_string(),
            tool_name: "Bash".to_string(),
        })
        .await;
    // The worker cites the runtime-issued id `ev-call-1`.
    handle
        .report_turn(GoalTurnDisposition::CompletionCandidate {
            coverage: satisfied_coverage(),
            evidence: vec![cite("ev-call-1")],
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(1, 1, true).await;
    // Ownership resolves, the gate authorizes, and the goal completes.
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    assert!(continuation.transition.expect("transition cell").met);
    assert_eq!(
        handle.snapshot().await.unwrap().status(),
        GoalStatus::Completed
    );
}

#[tokio::test]
async fn test_cited_unminted_evidence_is_rejected_and_continues() {
    let handle = handle();
    create(&handle).await;
    handle.bind_turn("t0".to_string()).await;
    // No evidence was minted; a fabricated id must fail ownership closed.
    handle
        .report_turn(GoalTurnDisposition::CompletionCandidate {
            coverage: satisfied_coverage(),
            evidence: vec![cite("ev-fabricated")],
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(1, 1, true).await;
    // The gate rejects on ownership; finalize returns Stop (§10.3, driver-owned
    // continuation) while the goal stays active+queued rather than completing.
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    assert_eq!(
        handle.snapshot().await.unwrap().status(),
        GoalStatus::Active
    );
}

#[tokio::test]
async fn test_record_evidence_off_a_goal_turn_is_a_no_op() {
    let handle = handle();
    create(&handle).await;
    // No bind_turn → the goal is queued, not running: nothing owns provenance,
    // so a later cite of the derived id must not resolve.
    handle
        .record_tool_evidence(ToolEvidenceObservation {
            tool_use_id: "call-1".to_string(),
            tool_name: "Bash".to_string(),
        })
        .await;
    handle.bind_turn("t0".to_string()).await;
    handle
        .report_turn(GoalTurnDisposition::CompletionCandidate {
            coverage: satisfied_coverage(),
            evidence: vec![cite("ev-call-1")],
        })
        .await
        .unwrap();
    let continuation = handle.finalize_goal_turn(1, 1, true).await;
    // Unminted evidence fails ownership; finalize returns Stop (§10.3) with the
    // goal still active+queued.
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
    assert_eq!(
        handle.snapshot().await.unwrap().status(),
        GoalStatus::Active
    );
}

#[tokio::test]
async fn test_finalize_without_running_turn_stops() {
    let handle = handle();
    create(&handle).await;
    // No bind_turn → the goal is queued, not running.
    let continuation = handle.finalize_goal_turn(1, 1, false).await;
    assert_eq!(continuation.continuation, GoalContinuation::Stop);
}
