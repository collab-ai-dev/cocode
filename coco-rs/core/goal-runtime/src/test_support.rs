//! Shared builders for the crate's tests.

use coco_goals::{
    BoundedText, CompletionContract, CompletionPolicy, CreateGoal, DurableResultRef, EvidenceId,
    EvidenceRef, EvidenceSource, GoalBudget, GoalCommand, GoalEvidenceRecord, GoalId, GoalLeaseId,
    GoalObjective, GoalSnapshot, GoalTurnDisposition, GoalTurnTrigger, ProgressSignal,
    RequirementCoverage, RequirementResult, StartTurn, Timestamp, VerificationAttemptId, WakeId,
    decide,
};
use coco_types::{SessionId, TurnId};

use crate::GoalTurnResult;

pub fn sid() -> SessionId {
    SessionId::try_new("sess-1").unwrap()
}

pub fn goal_id() -> GoalId {
    GoalId::new("g-1")
}

pub fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

pub fn create_cmd(policy: CompletionPolicy, contract: Option<CompletionContract>) -> GoalCommand {
    GoalCommand::Create(CreateGoal {
        goal_id: goal_id(),
        session_id: sid(),
        lease_id: GoalLeaseId::new("l0"),
        objective: GoalObjective::new("ship the feature"),
        contract,
        policy,
        budget: GoalBudget::default(),
        plan: None,
        mode_gate: None,
        wake_id: WakeId::new("w0"),
        at: ts(0),
    })
}

/// Active(queued l0) snapshot with the given policy.
pub fn queued_snapshot(policy: CompletionPolicy) -> GoalSnapshot {
    decide(None, create_cmd(policy, None))
        .unwrap()
        .snapshot
        .unwrap()
}

/// Active(running l0/t0) snapshot with the given policy and optional contract.
pub fn running_snapshot(
    policy: CompletionPolicy,
    contract: Option<CompletionContract>,
) -> GoalSnapshot {
    let created = decide(None, create_cmd(policy, contract))
        .unwrap()
        .snapshot
        .unwrap();
    decide(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: GoalLeaseId::new("l0"),
            turn_id: TurnId::new("t0"),
            trigger: GoalTurnTrigger::Creation,
            at: ts(1),
        }),
    )
    .unwrap()
    .snapshot
    .unwrap()
}

pub fn evidence_record(id: &str, owner: &GoalId) -> GoalEvidenceRecord {
    GoalEvidenceRecord {
        evidence_id: EvidenceId::new(id),
        goal_id: owner.clone(),
        lease_id: GoalLeaseId::new("l0"),
        turn_id: TurnId::new("t0"),
        source: EvidenceSource::ToolResult {
            tool: "Bash".to_string(),
        },
        result_ref: DurableResultRef::new("loc"),
        content_digest: None,
        observed_at: ts(1),
    }
}

pub fn evidence_ref(id: &str) -> EvidenceRef {
    EvidenceRef {
        evidence_id: EvidenceId::new(id),
        summary: BoundedText::short("ran test"),
    }
}

pub fn satisfied_coverage() -> RequirementCoverage {
    RequirementCoverage {
        requirements: vec![RequirementResult {
            requirement: BoundedText::short("done"),
            satisfied: true,
            evidence: Vec::new(),
        }],
        asserts_complete: true,
    }
}

pub fn turn_result(
    disposition: GoalTurnDisposition,
    signals: Vec<ProgressSignal>,
) -> GoalTurnResult {
    GoalTurnResult {
        disposition,
        signals,
        next_lease_id: GoalLeaseId::new("l1"),
        wake_id: WakeId::new("w1"),
        verification_attempt: VerificationAttemptId::new("va-1"),
        at: ts(2),
    }
}
