use super::*;
use pretty_assertions::assert_eq;

/// Build a plan revision `n` steps past the initial.
fn rev(steps: u64) -> PlanRevision {
    (0..steps).fold(PlanRevision::INITIAL, |r, _| r.next())
}

fn plan_ref(revision: PlanRevision, digest: Option<&str>) -> GoalPlanRef {
    GoalPlanRef {
        artifact_id: PlanArtifactId::new("plan-1"),
        revision,
        content_digest: digest.map(ContentDigest::new),
        observed_at: Timestamp::from_millis(0),
    }
}

#[test]
fn test_no_drift_when_same_revision_and_digest() {
    let current = plan_ref(rev(2), Some("abc"));
    let observed = plan_ref(rev(2), Some("abc"));
    assert!(!current.drifted_from(&observed));
}

#[test]
fn test_drift_on_digest_change() {
    let current = plan_ref(rev(2), Some("def"));
    let observed = plan_ref(rev(2), Some("abc"));
    assert!(current.drifted_from(&observed));
}

#[test]
fn test_drift_on_revision_change() {
    let current = plan_ref(rev(3), Some("abc"));
    let observed = plan_ref(rev(2), Some("abc"));
    assert!(current.drifted_from(&observed));
}

#[test]
fn test_no_drift_across_different_artifacts() {
    // Drift is only meaningful within one artifact id.
    let current = plan_ref(rev(2), Some("abc"));
    let mut observed = plan_ref(rev(9), Some("zzz"));
    observed.artifact_id = PlanArtifactId::new("other");
    assert!(!current.drifted_from(&observed));
}

#[test]
fn test_plan_checkpoint_roundtrip() {
    let checkpoint = PlanCheckpoint {
        revision: rev(1),
        content_digest: Some(ContentDigest::new("h")),
        summary: BoundedText::short("did work"),
        observed_at: Timestamp::from_millis(5),
    };
    let json = serde_json::to_string(&checkpoint).unwrap();
    let back: PlanCheckpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(back, checkpoint);
}
