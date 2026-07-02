use pretty_assertions::assert_eq;

use super::*;

const NOW: &str = "2026-07-02T00:00:00+00:00";

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = coco_skills::agent_scope::agent_skills_dir(tmp.path());
    std::fs::create_dir_all(&root).unwrap();
    (tmp, root)
}

#[test]
fn stamps_missing_provenance_and_records_patch() {
    let (tmp, root) = setup();
    let skill_md = root.join("learned").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    std::fs::write(&skill_md, "---\ndescription: d\n---\n# learned\nbody\n").unwrap();

    let stamped = stamp_written_skills(&root, tmp.path(), std::slice::from_ref(&skill_md), NOW);
    assert_eq!(stamped, 1);

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
        stamp_written_skills(&root, tmp.path(), std::slice::from_ref(&skill_md), NOW),
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
fn already_stamped_file_is_untouched() {
    let (tmp, root) = setup();
    let skill_md = root.join("done").join("SKILL.md");
    std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
    let content = format!(
        "---\ndescription: d\norigin: agent\ncreated-by: review\ncreated-at: {NOW}\n---\n# done\n"
    );
    std::fs::write(&skill_md, &content).unwrap();
    assert_eq!(
        stamp_written_skills(&root, tmp.path(), std::slice::from_ref(&skill_md), NOW),
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
        stamp_written_skills(&root, tmp.path(), std::slice::from_ref(&reference), NOW),
        0
    );
    assert_eq!(std::fs::read_to_string(&reference).unwrap(), "notes\n");
    let stats = coco_skills::telemetry::load_all(tmp.path());
    assert_eq!(stats.get("umbrella").unwrap().patch_count, 1);
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
        stamp_written_skills(&root, tmp.path(), std::slice::from_ref(&outside), NOW),
        0
    );
    let fm = coco_frontmatter::parse(&std::fs::read_to_string(&outside).unwrap());
    assert!(!fm.data.contains_key("origin"), "no stamp outside the root");
    assert!(coco_skills::telemetry::load_all(tmp.path()).is_empty());
}
