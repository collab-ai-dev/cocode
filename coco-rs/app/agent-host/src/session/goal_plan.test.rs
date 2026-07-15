use coco_goals::{PlanRevision, Timestamp};

use super::*;

fn write_plan(dir: &std::path::Path, body: &str) -> PathBuf {
    let path = dir.join("plan.md");
    std::fs::write(&path, body).expect("write plan");
    path
}

#[test]
fn resolves_headings_and_active_steps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_plan(
        dir.path(),
        "# Approach\nsome prose\n## Steps\n- [ ] write the parser\n- [x] done item\n- [ ] wire the engine\n",
    );
    let plan_ref = current_plan_ref("sess", &path, Timestamp::from_millis(1)).expect("ref");
    let source = SessionPlanSource::new(path);

    let view = source.plan_view(&plan_ref).expect("ok").expect("view");
    assert_eq!(view.headings, vec!["Approach", "Steps"]);
    assert_eq!(
        view.active_steps,
        vec!["write the parser", "wire the engine"]
    );
    assert!(!view.drifted);
}

#[test]
fn detects_digest_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_plan(dir.path(), "# Plan\n- [ ] step one\n");
    let plan_ref = current_plan_ref("sess", &path, Timestamp::from_millis(1)).expect("ref");

    // Edit the file so its digest differs from the bound reference.
    std::fs::write(&path, "# Plan\n- [ ] step one\n- [ ] step two\n").expect("rewrite");
    let source = SessionPlanSource::new(path);
    let view = source.plan_view(&plan_ref).expect("ok").expect("view");
    assert!(view.drifted, "changed file must report drift");
}

#[test]
fn missing_plan_degrades_to_path_only_view() {
    let plan_ref = GoalPlanRef {
        artifact_id: session_plan_artifact_id("sess"),
        revision: PlanRevision::INITIAL,
        content_digest: None,
        observed_at: Timestamp::from_millis(1),
    };
    let source = SessionPlanSource::new(PathBuf::from("/no/such/plan.md"));
    let view = source.plan_view(&plan_ref).expect("ok").expect("view");
    // Never suppressed — reminder still renders with an empty plan section.
    assert!(view.digest.is_none());
    assert!(view.active_steps.is_empty());
    assert!(!view.drifted);
}

#[test]
fn no_plan_file_yields_no_ref() {
    assert!(
        current_plan_ref(
            "sess",
            std::path::Path::new("/no/such/plan.md"),
            Timestamp::from_millis(1)
        )
        .is_none()
    );
}
