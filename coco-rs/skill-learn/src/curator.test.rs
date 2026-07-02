use std::path::Path;

use coco_skills::agent_scope::agent_skills_dir;
use coco_skills::telemetry::{SkillOutcome, record_invocation};
use pretty_assertions::assert_eq;

use super::{CuratorOutcome, SkillCurator};

fn write_agent_skill(agent_root: &Path, name: &str) {
    let dir = agent_root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let md = format!(
        "---\ndescription: {name} skill\norigin: agent\ncreated-by: review\n---\n# {name}\n\nbody\n"
    );
    std::fs::write(dir.join("SKILL.md"), md).unwrap();
}

fn is_disabled(agent_root: &Path, name: &str) -> bool {
    let content = std::fs::read_to_string(agent_root.join(name).join("SKILL.md")).unwrap();
    coco_frontmatter::parse(&content)
        .data
        .get("disabled")
        .and_then(coco_frontmatter::FrontmatterValue::as_bool)
        .unwrap_or(false)
}

#[test]
fn retires_misfiring_skill_promotes_good_one() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let agent_root = agent_skills_dir(home);
    write_agent_skill(&agent_root, "misfire");
    write_agent_skill(&agent_root, "good");

    // misfire: 1 success / 9 failures = 10% over 10 runs → below 34% gate.
    record_invocation(home, "misfire", SkillOutcome::Success);
    for _ in 0..9 {
        record_invocation(home, "misfire", SkillOutcome::Failure);
    }
    // good: 5 successes → 100%, above the 80% promotion gate.
    for _ in 0..5 {
        record_invocation(home, "good", SkillOutcome::Success);
    }

    let curator = SkillCurator::new(home);
    let outcome = curator.maybe_curate();
    assert_eq!(
        outcome,
        CuratorOutcome::Ran {
            retired: 1,
            promoted: 1,
            scanned: 2
        }
    );
    assert!(
        is_disabled(&agent_root, "misfire"),
        "misfire must be retired"
    );
    assert!(!is_disabled(&agent_root, "good"), "good must stay enabled");
    assert!(
        coco_skills::agent_scope::load_promotions(home).contains("good"),
        "good must be promoted"
    );
}

#[test]
fn scan_is_location_keyed_unstamped_artifact_still_curated() {
    // Regression: eligibility must NOT key on the LLM-written `origin: agent`
    // frontmatter — an artifact missing the stamp would otherwise be immortal.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let agent_root = agent_skills_dir(home);
    let dir = agent_root.join("unstamped");
    std::fs::create_dir_all(&dir).unwrap();
    // No `origin:` key at all.
    std::fs::write(dir.join("SKILL.md"), "---\ndescription: d\n---\n# u\n").unwrap();
    for _ in 0..5 {
        record_invocation(home, "unstamped", SkillOutcome::Failure);
    }

    let outcome = SkillCurator::new(home).maybe_curate();
    assert_eq!(
        outcome,
        CuratorOutcome::Ran {
            retired: 1,
            promoted: 0,
            scanned: 1
        }
    );
    assert!(is_disabled(&agent_root, "unstamped"));
}

#[test]
fn does_not_gate_below_min_invocations() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let agent_root = agent_skills_dir(home);
    write_agent_skill(&agent_root, "young");
    // Only 2 invocations (below the 5 floor), both failures.
    record_invocation(home, "young", SkillOutcome::Failure);
    record_invocation(home, "young", SkillOutcome::Failure);

    let outcome = SkillCurator::new(home).maybe_curate();
    assert_eq!(
        outcome,
        CuratorOutcome::Ran {
            retired: 0,
            promoted: 0,
            scanned: 1
        }
    );
    assert!(!is_disabled(&agent_root, "young"));
}

#[test]
fn promotion_is_idempotent_across_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let agent_root = agent_skills_dir(home);
    write_agent_skill(&agent_root, "good");
    for _ in 0..5 {
        record_invocation(home, "good", SkillOutcome::Success);
    }

    let first = SkillCurator::new(home).with_min_hours(0).maybe_curate();
    assert_eq!(
        first,
        CuratorOutcome::Ran {
            retired: 0,
            promoted: 1,
            scanned: 1
        }
    );
    // min_hours 0 bypasses the time gate; second pass must not re-promote.
    let second = SkillCurator::new(home).with_min_hours(0).maybe_curate();
    assert_eq!(
        second,
        CuratorOutcome::Ran {
            retired: 0,
            promoted: 0,
            scanned: 1
        }
    );
}

#[test]
fn second_pass_hits_time_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_agent_skill(&agent_skills_dir(home), "s");
    let curator = SkillCurator::new(home);
    // First pass runs (no prior lock mtime), stamps lastConsolidatedAt=now.
    assert!(matches!(curator.maybe_curate(), CuratorOutcome::Ran { .. }));
    // Immediate second pass is inside the 24h window.
    assert_eq!(curator.maybe_curate(), CuratorOutcome::SkippedTimeGate);
}

#[test]
fn curator_lock_lives_outside_the_fenced_root() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_agent_skill(&agent_skills_dir(home), "s");
    assert!(matches!(
        SkillCurator::new(home).maybe_curate(),
        CuratorOutcome::Ran { .. }
    ));
    let lock = home.join("skills").join(".skill-curator-lock");
    assert!(lock.exists(), "lock stamped at <config_home>/skills");
    assert!(
        !lock.starts_with(agent_skills_dir(home)),
        "a fork fenced to .agent must not be able to touch the curator lock"
    );
}
