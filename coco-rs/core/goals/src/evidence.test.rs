use super::*;
use coco_types::TurnId;
use pretty_assertions::assert_eq;

fn record(goal: &str) -> GoalEvidenceRecord {
    GoalEvidenceRecord {
        evidence_id: EvidenceId::new("e-1"),
        goal_id: GoalId::new(goal),
        lease_id: GoalLeaseId::new("l-1"),
        turn_id: TurnId::new("t-1"),
        source: EvidenceSource::ToolResult {
            tool: "Bash".to_string(),
        },
        result_ref: DurableResultRef::new("transcript://t-1/tool/0"),
        content_digest: Some(ContentDigest::new("h")),
        observed_at: Timestamp::from_millis(1),
    }
}

#[test]
fn test_owned_by_matches_producing_goal() {
    let record = record("g-1");
    assert!(record.owned_by(&GoalId::new("g-1")));
    assert!(!record.owned_by(&GoalId::new("g-2")));
}

#[test]
fn test_evidence_record_roundtrip() {
    let record = record("g-1");
    let json = serde_json::to_string(&record).unwrap();
    let back: GoalEvidenceRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, record);
}

#[test]
fn test_evidence_source_variants_serialize_snake_case() {
    let source = EvidenceSource::DeterministicCheck {
        check: BoundedText::short("just test"),
    };
    let json = serde_json::to_value(&source).unwrap();
    assert_eq!(json["kind"], "deterministic_check");
}
