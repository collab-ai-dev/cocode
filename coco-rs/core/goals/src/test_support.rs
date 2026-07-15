//! Shared builders for the crate's reducer/value-object tests.

use coco_types::{SessionId, TurnId};

use crate::*;

pub fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

pub fn sid() -> SessionId {
    SessionId::try_new("sess-1").unwrap()
}

pub fn goal_id() -> GoalId {
    GoalId::new("g-1")
}

pub fn lease(name: &str) -> GoalLeaseId {
    GoalLeaseId::new(name)
}

pub fn turn(name: &str) -> TurnId {
    TurnId::new(name)
}

pub fn wake(name: &str) -> WakeId {
    WakeId::new(name)
}

/// Unwrap a successful decision's snapshot.
pub fn apply(snapshot: Option<&GoalSnapshot>, command: GoalCommand) -> GoalDecision {
    decide(snapshot, command).expect("decision should succeed")
}

pub fn next_snapshot(snapshot: Option<&GoalSnapshot>, command: GoalCommand) -> GoalSnapshot {
    apply(snapshot, command)
        .snapshot
        .expect("decision should retain a snapshot")
}

pub fn create_cmd() -> CreateGoal {
    CreateGoal {
        goal_id: goal_id(),
        session_id: sid(),
        lease_id: lease("l0"),
        objective: GoalObjective::new("ship the feature"),
        contract: None,
        policy: CompletionPolicy::CandidateWithEvidence,
        budget: GoalBudget::default(),
        plan: None,
        mode_gate: None,
        wake_id: wake("w0"),
        at: ts(0),
    }
}

/// Snapshot right after creation: `active(queued l0)`.
pub fn created_snapshot() -> GoalSnapshot {
    next_snapshot(None, GoalCommand::Create(create_cmd()))
}

/// Snapshot with a running turn under lease `l0` / turn `t0` (creation trigger).
pub fn running_snapshot() -> GoalSnapshot {
    let created = created_snapshot();
    next_snapshot(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Creation,
            at: ts(1),
        }),
    )
}

/// A minimal satisfied completion candidate citing no evidence.
pub fn simple_completion_candidate() -> CompletionCandidate {
    CompletionCandidate {
        source: CandidateSource::WorkerReport,
        coverage: RequirementCoverage {
            requirements: vec![RequirementResult {
                requirement: BoundedText::short("done"),
                satisfied: true,
                evidence: Vec::new(),
            }],
            asserts_complete: true,
        },
        evidence: Vec::new(),
        plan_observed: None,
    }
}

/// Mint a real completion authorization for the current goal/spec/running lease.
pub fn authorization_for(
    snapshot: &GoalSnapshot,
    running_lease: &GoalLeaseId,
) -> CompletionAuthorization {
    let candidate = simple_completion_candidate();
    let outcome = authorize_completion(
        &snapshot.goal_id,
        snapshot.spec_revision,
        running_lease,
        snapshot.plan.as_ref(),
        &candidate,
        &[],
        VerificationOutcome::Verified {
            summary: CompletionEvidenceSummary {
                summary: BoundedText::short("verified"),
                verified_requirements: Vec::new(),
                cited_evidence: Vec::new(),
            },
        },
    );
    match outcome {
        CompletionOutcome::Authorized(auth) => auth,
        other => panic!("expected authorization, got {other:?}"),
    }
}
