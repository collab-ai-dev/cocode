use super::*;
use coco_skills::SkillSource;
use coco_skills::telemetry::{SkillOutcome, record_invocation};
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::path::Path;

/// Promotion threshold these tests build snapshots with. Deliberately NOT the
/// production default (5), so a regression that reintroduces a hardcoded
/// threshold fails here instead of coincidentally matching.
const TEST_PROMOTE_MIN: i64 = 7;

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Write an agent skill under `<config_home>/skills/.agent/<name>/SKILL.md`.
fn agent_skill(config_home: &Path, name: &str, disabled: bool, created_at: Option<&str>) {
    let mut fm = String::from("---\norigin: agent\ncreated-by: review\n");
    if let Some(c) = created_at {
        fm.push_str(&format!("created-at: {c}\n"));
    }
    if disabled {
        fm.push_str("disabled: true\n");
    }
    fm.push_str(&format!(
        "description: agent skill {name}\n---\n# {name}\n\nbody\n"
    ));
    write(
        &config_home
            .join("skills/.agent")
            .join(name)
            .join("SKILL.md"),
        &fm,
    );
}

/// Build an `Arc<SkillDefinition>` from a written user-scope SKILL.md, then
/// override its source (so bundled/managed cases are exercisable).
fn user_skill(dir: &Path, name: &str, source: SkillSource) -> std::sync::Arc<SkillDefinition> {
    let path = dir.join(name).join("SKILL.md");
    write(
        &path,
        &format!("---\ndescription: user skill {name}\n---\n# {name}\n\nbody\n"),
    );
    let mut s = coco_skills::load_skill_from_file(&path).unwrap();
    s.source = source;
    std::sync::Arc::new(s)
}

fn promote(config_home: &Path, name: &str) {
    let mut set = HashSet::new();
    set.insert(name.to_string());
    assert!(coco_skills::agent_scope::save_promotions(config_home, &set));
}

fn lifecycle_of(snap: &JourneySnapshot, title: &str) -> Option<AgentSkillLifecycle> {
    snap.nodes.iter().find(|n| n.title == title).and_then(|n| {
        if let JourneyNodeBody::AgentSkill { lifecycle, .. } = &n.body {
            Some(*lifecycle)
        } else {
            None
        }
    })
}

#[test]
fn test_empty_dirs_yield_empty_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = JourneyPaths {
        config_home: tmp.path().join("nonexistent"),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);
    assert!(snap.nodes.is_empty());
    assert_eq!(snap.stats, JourneyStats::default());
}

#[test]
fn test_agent_skill_lifecycle_matrix() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    agent_skill(home, "learning-one", false, None);
    agent_skill(home, "learned-one", false, None);
    agent_skill(home, "retired-one", true, None);
    promote(home, "learned-one");

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);

    // Never invoked ⇒ 0 progress, and `required` must echo the caller's
    // configured threshold rather than any built-in default.
    assert_eq!(
        lifecycle_of(&snap, "learning-one"),
        Some(AgentSkillLifecycle::Learning {
            invocations: 0,
            required: TEST_PROMOTE_MIN,
        })
    );
    assert_eq!(
        lifecycle_of(&snap, "learned-one"),
        Some(AgentSkillLifecycle::Learned)
    );
    assert_eq!(
        lifecycle_of(&snap, "retired-one"),
        Some(AgentSkillLifecycle::Retired)
    );
    assert_eq!(snap.stats.learning, 1);
    assert_eq!(snap.stats.learned, 1);
    assert_eq!(snap.stats.retired, 1);
}

#[test]
fn test_user_skill_filter_bundled_and_unused_excluded() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let user_dir = tmp.path().join("user");

    let used = user_skill(
        &user_dir,
        "used-skill",
        SkillSource::User {
            path: user_dir.join("used-skill/SKILL.md"),
        },
    );
    let unused = user_skill(
        &user_dir,
        "unused-skill",
        SkillSource::User {
            path: user_dir.join("unused-skill/SKILL.md"),
        },
    );
    let bundled = user_skill(&user_dir, "bundled-skill", SkillSource::Bundled);

    // Only `used-skill` has telemetry.
    record_invocation(home, "used-skill", SkillOutcome::Success);
    // Give the bundled one telemetry too — it must STILL be excluded by source.
    record_invocation(home, "bundled-skill", SkillOutcome::Success);

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[used, unused, bundled], TEST_PROMOTE_MIN);

    let titles: Vec<&str> = snap.nodes.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["used-skill"]);
    assert_eq!(snap.stats.user_skills, 1);
}

#[test]
fn test_memory_nodes_from_memdir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let memdir = tmp.path().join("mem");
    write(
        &memdir.join("topic-a.md"),
        "---\nname: Topic A\ndescription: first topic\ntype: reference\n---\nbody\n",
    );
    write(
        &memdir.join("topic-b.md"),
        "---\nname: Topic B\ndescription: second topic\ntype: project\n---\nbody\n",
    );
    // MEMORY.md index must be excluded from the node list.
    write(
        &memdir.join("MEMORY.md"),
        "# index\n- [Topic A](topic-a.md)\n",
    );

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: Some(memdir),
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);

    assert_eq!(snap.stats.memories, 2);
    let mut titles: Vec<&str> = snap
        .nodes
        .iter()
        .filter(|n| matches!(n.body, JourneyNodeBody::Memory { .. }))
        .map(|n| n.title.as_str())
        .collect();
    titles.sort();
    assert_eq!(titles, vec!["Topic A", "Topic B"]);
}

#[test]
fn test_provenance_created_at_beats_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // A provenance timestamp far in the past; the file's mtime is ~now.
    agent_skill(home, "old-skill", false, Some("2020-01-01T00:00:00Z"));

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);
    let node = snap.nodes.iter().find(|n| n.title == "old-skill").unwrap();
    // 2020-01-01 in ms.
    let expected = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(node.first_seen_ms, expected);
}

#[test]
fn test_human_skill_falls_back_to_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let user_dir = tmp.path().join("user");
    let skill = user_skill(
        &user_dir,
        "human",
        SkillSource::User {
            path: user_dir.join("human/SKILL.md"),
        },
    );
    record_invocation(home, "human", SkillOutcome::Success);

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[skill], TEST_PROMOTE_MIN);
    let node = snap.nodes.iter().find(|n| n.title == "human").unwrap();
    // mtime is a recent, positive timestamp.
    assert!(node.first_seen_ms > 0);
}

#[test]
fn test_journal_first_seen_beats_provenance_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    agent_skill(home, "j-skill", false, Some("2020-01-01T00:00:00Z"));
    // A SkillLearned dated before the provenance created-at.
    let learned_ms = chrono::DateTime::parse_from_rfc3339("2019-06-01T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let journal = coco_skills::agent_scope::agent_journal_path(home);
    coco_maintenance::journal::append_jsonl(
        &journal,
        &coco_types::JourneyRecord::new(
            learned_ms,
            Some("sess".into()),
            coco_types::JourneyEvent::SkillLearned {
                name: "j-skill".into(),
            },
        ),
    );

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);
    let node = snap.nodes.iter().find(|n| n.title == "j-skill").unwrap();
    assert_eq!(node.first_seen_ms, learned_ms);
    assert_eq!(node.history.len(), 1);
}

#[test]
fn test_memory_journal_history_attached() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let memdir = tmp.path().join("mem");
    write(
        &memdir.join("t.md"),
        "---\nname: T\ndescription: d\ntype: reference\n---\nx\n",
    );
    let journal = coco_memory::path::memory_journal_path(&memdir);
    coco_maintenance::journal::append_jsonl(
        &journal,
        &coco_types::JourneyRecord::new(
            1000,
            Some("sess".into()),
            coco_types::JourneyEvent::MemoryWritten {
                files: vec!["t.md".into()],
            },
        ),
    );

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: Some(memdir),
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);
    let node = snap
        .nodes
        .iter()
        .find(|n| matches!(n.body, JourneyNodeBody::Memory { .. }))
        .unwrap();
    assert_eq!(node.history.len(), 1);
    assert_eq!(node.first_seen_ms, 1000);
}

#[test]
fn test_nodes_sorted_ascending_by_last_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    agent_skill(home, "a", false, None);
    agent_skill(home, "b", false, None);
    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);
    for w in snap.nodes.windows(2) {
        assert!(w[0].last_activity_ms <= w[1].last_activity_ms);
    }
}

#[test]
fn learning_progress_counts_invocations_not_successes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    agent_skill(home, "mixed-one", false, None);
    record_invocation(home, "mixed-one", SkillOutcome::Success);
    record_invocation(home, "mixed-one", SkillOutcome::Failure);
    record_invocation(home, "mixed-one", SkillOutcome::Failure);

    let paths = JourneyPaths {
        config_home: home.to_path_buf(),
        memdir: None,
    };
    let snap = build_journey(&paths, &[], TEST_PROMOTE_MIN);

    // The curator's first gate is `total_invocations() >= promote_min_invocations`
    // (successes + failures), so progress must be 3 here. Reporting the success
    // count (1) would under-report real progress toward promotion.
    assert_eq!(
        lifecycle_of(&snap, "mixed-one"),
        Some(AgentSkillLifecycle::Learning {
            invocations: 3,
            required: TEST_PROMOTE_MIN,
        })
    );
}
