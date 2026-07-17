use pretty_assertions::assert_eq;

use super::*;

const NOW: &str = "2026-07-02T00:00:00+00:00";

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = coco_skills::agent_scope::agent_skills_dir(tmp.path());
    std::fs::create_dir_all(&root).unwrap();
    (tmp, root)
}

/// Run one stamping pass over `paths`.
///
/// `pre_existing` names the skills the host observed under the root *before*
/// the fork ran — the trusted basis for Learned vs Updated. An empty set means
/// "the fork created everything it wrote".
fn stamp(
    root: &std::path::Path,
    config_home: &std::path::Path,
    paths: &[std::path::PathBuf],
    session_id: Option<&str>,
    author: SkillAuthor,
    pre_existing: &[&str],
) -> StampOutcome {
    let pre_existing: std::collections::HashSet<String> =
        pre_existing.iter().map(|s| (*s).to_string()).collect();
    stamp_written_skills(StampRequest {
        agent_root: root,
        config_home,
        paths_written: paths,
        now_rfc3339: NOW,
        session_id,
        author,
        pre_existing: &pre_existing,
        journal_enabled: true,
    })
}

#[test]
fn stamps_missing_provenance_and_records_patch() {
    let (tmp, root) = setup();
    let skill_md = root.join("learned").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(&skill_md, "---\ndescription: d\n---\n# learned\nbody\n").unwrap();

    let outcome = stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        None,
        SkillAuthor::Review,
        &[],
    );
    assert_eq!(outcome.stamped, 1);

    let content = std::fs::read_to_string(&skill_md).unwrap();
    let fm = coco_frontmatter::parse(&content);
    assert_eq!(
        fm.data.get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );
    assert_eq!(
        fm.data.get("created-by").and_then(|v| v.as_str()),
        Some("review")
    );
    assert_eq!(
        fm.data.get("created-at").and_then(|v| v.as_str()),
        Some(NOW)
    );
    assert!(content.contains("# learned"), "body preserved");

    let stats = coco_skills::telemetry::load_all(tmp.path());
    assert_eq!(stats.get("learned").unwrap().patch_count, 1);
}

#[test]
fn spoofed_user_origin_is_overwritten_but_creation_record_kept() {
    let (tmp, root) = setup();
    let skill_md = root.join("spoof").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(
        &skill_md,
        "---\ndescription: d\norigin: user\ncreated-by: curator\ncreated-at: 2025-01-01T00:00:00+00:00\n---\n# spoof\n",
    )
    .unwrap();

    assert_eq!(
        stamp(
            &root,
            tmp.path(),
            std::slice::from_ref(&skill_md),
            None,
            SkillAuthor::Review,
            // The host saw this skill before the fork ⇒ an UPDATE.
            &["spoof"],
        )
        .stamped,
        1
    );
    let fm = coco_frontmatter::parse(&std::fs::read_to_string(&skill_md).unwrap());
    assert_eq!(
        fm.data.get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );
    // Existing authorship record is preserved (only origin is force-set).
    assert_eq!(
        fm.data.get("created-by").and_then(|v| v.as_str()),
        Some("curator")
    );
    assert_eq!(
        fm.data.get("created-at").and_then(|v| v.as_str()),
        Some("2025-01-01T00:00:00+00:00")
    );
}

#[test]
fn fork_claimed_created_at_cannot_disguise_a_birth_as_an_update() {
    let (tmp, root) = setup();
    let skill_md = root.join("sneaky").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    // The fork writes a brand-new skill but backdates it and claims the curator
    // authored it. Frontmatter is LLM-written, so none of it is evidence.
    std::fs::write(
        &skill_md,
        "---\ndescription: d\ncreated-by: curator\ncreated-at: 2020-01-01T00:00:00+00:00\n---\n# sneaky\n",
    )
    .unwrap();

    let outcome = stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        None,
        SkillAuthor::Review,
        // Host saw nothing before the fork ⇒ this IS a birth, whatever the file says.
        &[],
    );

    let fm = coco_frontmatter::parse(&std::fs::read_to_string(&skill_md).unwrap());
    assert_eq!(
        fm.data.get("created-at").and_then(|v| v.as_str()),
        Some(NOW),
        "a birth's timestamp is ours to state; the fork cannot backdate the timeline"
    );
    assert_eq!(
        fm.data.get("created-by").and_then(|v| v.as_str()),
        Some("review"),
        "authorship on a birth is force-set, not taken from the file"
    );
    // And the journal must carry a creation event, not an update — otherwise the
    // skill enters the timeline with no birth and skips the quarantine notice.
    assert_eq!(
        outcome.notices.first().map(|n| n.verb),
        Some(SkillLearnVerb::Learned)
    );
    let journal = coco_skills::agent_scope::agent_journal_path(tmp.path());
    let records: Vec<coco_types::JourneyRecord> = coco_maintenance::journal::read_jsonl(&journal);
    assert!(matches!(
        records[0].event,
        coco_types::JourneyEvent::SkillLearned { .. }
    ));
}

#[test]
fn already_stamped_file_is_untouched() {
    let (tmp, root) = setup();
    let skill_md = root.join("done").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    let content = format!(
        "---\ndescription: d\norigin: agent\ncreated-by: review\ncreated-at: {NOW}\n---\n# done\n"
    );
    std::fs::write(&skill_md, &content).unwrap();
    assert_eq!(
        stamp(
            &root,
            tmp.path(),
            std::slice::from_ref(&skill_md),
            None,
            SkillAuthor::Review,
            &[],
        )
        .stamped,
        0
    );
    // Patch telemetry still records the write the fork made.
    let stats = coco_skills::telemetry::load_all(tmp.path());
    assert_eq!(stats.get("done").unwrap().patch_count, 1);
}

#[test]
fn support_files_count_for_patch_but_are_not_stamped() {
    let (tmp, root) = setup();
    let reference = root.join("umbrella").join("reference.md");
    std::fs::create_dir_all(reference.parent().unwrap()).unwrap();
    std::fs::write(&reference, "notes\n").unwrap();

    assert_eq!(
        stamp(
            &root,
            tmp.path(),
            std::slice::from_ref(&reference),
            None,
            SkillAuthor::Review,
            &[],
        )
        .stamped,
        0
    );
    assert_eq!(std::fs::read_to_string(&reference).unwrap(), "notes\n");
    let stats = coco_skills::telemetry::load_all(tmp.path());
    assert_eq!(stats.get("umbrella").unwrap().patch_count, 1);
}

#[test]
fn overlong_description_truncated_at_char_boundary() {
    let (tmp, root) = setup();
    let skill_md = root.join("verbose").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    // A multibyte (CJK) description well over the 120-byte budget.
    let long = "描述".repeat(60); // 3 bytes/char * 120 chars = 360 bytes
    std::fs::write(
        &skill_md,
        format!("---\ndescription: {long}\n---\n# verbose\n"),
    )
    .unwrap();

    stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        None,
        SkillAuthor::Review,
        &[],
    );

    let content = std::fs::read_to_string(&skill_md).unwrap();
    let fm = coco_frontmatter::parse(&content);
    let desc = fm.data.get("description").and_then(|v| v.as_str()).unwrap();
    assert!(desc.len() < long.len(), "description was truncated");
    assert!(desc.ends_with("..."), "ellipsis appended");
}

#[test]
fn manual_author_is_stamped_for_user_initiated_learn() {
    let (tmp, root) = setup();
    let skill_md = root.join("manual-skill").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(&skill_md, "---\ndescription: d\n---\n# manual-skill\n").unwrap();

    stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        None,
        SkillAuthor::Manual,
        &[],
    );

    let fm = coco_frontmatter::parse(&std::fs::read_to_string(&skill_md).unwrap());
    assert_eq!(
        fm.data.get("created-by").and_then(|v| v.as_str()),
        Some("manual"),
        "/learn-created skills record manual authorship"
    );
    // Origin is still force-stamped agent (quarantine does not relax).
    assert_eq!(
        fm.data.get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );
}

#[test]
fn journal_records_learned_for_new_skill() {
    let (tmp, root) = setup();
    let skill_md = root.join("fresh").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(&skill_md, "---\ndescription: d\n---\n# fresh\n").unwrap();

    stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        Some("sess-1"),
        SkillAuthor::Review,
        &[],
    );

    let journal = coco_skills::agent_scope::agent_journal_path(tmp.path());
    let records: Vec<coco_types::JourneyRecord> = coco_maintenance::journal::read_jsonl(&journal);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id.as_deref(), Some("sess-1"));
    assert!(matches!(
        records[0].event,
        coco_types::JourneyEvent::SkillLearned { .. }
    ));
}

#[test]
fn journal_records_updated_for_existing_skill() {
    let (tmp, root) = setup();
    let skill_md = root.join("existing").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(
        &skill_md,
        format!(
            "---\ndescription: d\norigin: agent\ncreated-by: review\ncreated-at: {NOW}\n---\n# existing\n"
        ),
    )
    .unwrap();

    stamp(
        &root,
        tmp.path(),
        std::slice::from_ref(&skill_md),
        None,
        SkillAuthor::Review,
        // Present before the fork ⇒ an update, regardless of frontmatter.
        &["existing"],
    );

    let journal = coco_skills::agent_scope::agent_journal_path(tmp.path());
    let records: Vec<coco_types::JourneyRecord> = coco_maintenance::journal::read_jsonl(&journal);
    assert_eq!(records.len(), 1);
    assert!(matches!(
        records[0].event,
        coco_types::JourneyEvent::SkillUpdated { .. }
    ));
}

#[test]
fn journal_disabled_writes_no_records_but_still_stamps_and_notifies() {
    let (tmp, root) = setup();
    let skill_md = root.join("quiet").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(&skill_md, "---\ndescription: d\n---\n# quiet\n").unwrap();

    let pre_existing = std::collections::HashSet::new();
    let outcome = stamp_written_skills(StampRequest {
        agent_root: &root,
        config_home: tmp.path(),
        paths_written: std::slice::from_ref(&skill_md),
        now_rfc3339: NOW,
        session_id: None,
        author: SkillAuthor::Review,
        pre_existing: &pre_existing,
        journal_enabled: false,
    });

    // `journal_enabled` governs the journal only — provenance enforcement and
    // the user-visible notice are not observability and must not be gated.
    assert_eq!(outcome.stamped, 1);
    assert_eq!(outcome.notices.len(), 1);
    let journal = coco_skills::agent_scope::agent_journal_path(tmp.path());
    assert!(!journal.exists(), "no journal file when journaling is off");
}

#[test]
fn paths_outside_agent_root_are_ignored() {
    let (tmp, root) = setup();
    let outside = tmp
        .path()
        .join("skills")
        .join("user-skill")
        .join("SKILL.md");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "---\ndescription: d\n---\n# u\n").unwrap();

    assert_eq!(
        stamp(
            &root,
            tmp.path(),
            std::slice::from_ref(&outside),
            None,
            SkillAuthor::Review,
            &[],
        )
        .stamped,
        0
    );
    let fm = coco_frontmatter::parse(&std::fs::read_to_string(&outside).unwrap());
    assert!(!fm.data.contains_key("origin"), "no stamp outside the root");
    assert!(coco_skills::telemetry::load_all(tmp.path()).is_empty());
}

#[test]
fn existing_skill_names_snapshots_directories() {
    let (_tmp, root) = setup();
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::create_dir_all(root.join("beta")).unwrap();
    // A stray file at the root is not a skill.
    std::fs::write(root.join("notes.md"), "x").unwrap();

    let names = existing_skill_names(&root);
    assert_eq!(
        names,
        ["alpha", "beta"]
            .iter()
            .map(|s| (*s).to_string())
            .collect::<std::collections::HashSet<_>>()
    );
}

#[test]
fn existing_skill_names_is_empty_for_absent_root() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(existing_skill_names(&tmp.path().join("nope")).is_empty());
}
