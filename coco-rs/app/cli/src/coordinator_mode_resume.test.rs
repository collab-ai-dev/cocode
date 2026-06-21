use super::reconcile_on_resume;
use coco_types::Features;

#[test]
fn reconcile_no_ops_when_agent_teams_disabled() {
    // Without the AgentTeams gate the reconcile short-circuits before any
    // env mutation, regardless of the stored mode.
    let features = Features::empty();
    assert_eq!(reconcile_on_resume(Some("coordinator"), &features), None);
    assert_eq!(reconcile_on_resume(Some("normal"), &features), None);
    assert_eq!(reconcile_on_resume(None, &features), None);
}
