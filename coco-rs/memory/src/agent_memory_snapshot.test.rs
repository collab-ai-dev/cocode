use super::*;

use coco_types::MemoryScope;
use std::fs;

fn write_snapshot(cwd: &std::path::Path, agent_type: &str, ts: &str, files: &[(&str, &str)]) {
    let dir = snapshot_dir_for_agent(agent_type, cwd);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(SNAPSHOT_JSON),
        serde_json::to_string(&SnapshotMeta {
            updated_at: ts.to_string(),
        })
        .unwrap(),
    )
    .unwrap();
    for (name, body) in files {
        fs::write(dir.join(name), body).unwrap();
    }
}

#[test]
fn test_check_returns_none_when_no_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    let action = check_agent_memory_snapshot("Explore", MemoryScope::Project, cwd, &home);
    assert_eq!(action, SnapshotAction::None);
}

#[test]
fn test_check_returns_initialize_when_snapshot_present_but_no_local() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    write_snapshot(
        cwd,
        "Explore",
        "2026-01-01T00:00:00Z",
        &[("MEMORY.md", "- baseline")],
    );

    let action = check_agent_memory_snapshot("Explore", MemoryScope::Project, cwd, &home);
    assert_eq!(
        action,
        SnapshotAction::Initialize {
            snapshot_timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    );
}

#[test]
fn test_check_returns_none_when_synced_meta_matches() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    write_snapshot(
        cwd,
        "Explore",
        "2026-01-01T00:00:00Z",
        &[("MEMORY.md", "- baseline")],
    );
    initialize_from_snapshot(
        "Explore",
        MemoryScope::Project,
        "2026-01-01T00:00:00Z",
        cwd,
        &home,
    )
    .unwrap();

    let action = check_agent_memory_snapshot("Explore", MemoryScope::Project, cwd, &home);
    assert_eq!(action, SnapshotAction::None);
}

#[test]
fn test_check_returns_prompt_update_when_snapshot_newer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    write_snapshot(
        cwd,
        "Explore",
        "2026-01-01T00:00:00Z",
        &[("MEMORY.md", "- old")],
    );
    initialize_from_snapshot(
        "Explore",
        MemoryScope::Project,
        "2026-01-01T00:00:00Z",
        cwd,
        &home,
    )
    .unwrap();

    // Bump snapshot timestamp.
    write_snapshot(
        cwd,
        "Explore",
        "2026-02-01T00:00:00Z",
        &[("MEMORY.md", "- newer")],
    );

    let action = check_agent_memory_snapshot("Explore", MemoryScope::Project, cwd, &home);
    assert_eq!(
        action,
        SnapshotAction::PromptUpdate {
            snapshot_timestamp: "2026-02-01T00:00:00Z".to_string(),
        }
    );
}

#[test]
fn test_initialize_copies_md_files_skips_snapshot_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    write_snapshot(
        cwd,
        "Plan",
        "2026-01-01T00:00:00Z",
        &[("MEMORY.md", "- entry"), ("notes.md", "extra")],
    );

    initialize_from_snapshot(
        "Plan",
        MemoryScope::Project,
        "2026-01-01T00:00:00Z",
        cwd,
        &home,
    )
    .unwrap();

    let local_dir = agent_memory_dir("Plan", MemoryScope::Project, cwd, &home);
    assert!(local_dir.join("MEMORY.md").exists());
    assert!(local_dir.join("notes.md").exists());
    assert!(
        !local_dir.join(SNAPSHOT_JSON).exists(),
        "snapshot.json must NOT be copied into the local memory dir"
    );
}

#[test]
fn test_replace_wipes_existing_md_then_copies() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    let local_dir = agent_memory_dir("Plan", MemoryScope::Project, cwd, &home);
    fs::create_dir_all(&local_dir).unwrap();
    fs::write(local_dir.join("MEMORY.md"), "stale").unwrap();
    fs::write(local_dir.join("orphan.md"), "should be wiped").unwrap();

    write_snapshot(
        cwd,
        "Plan",
        "2026-02-01T00:00:00Z",
        &[("MEMORY.md", "fresh")],
    );

    replace_from_snapshot(
        "Plan",
        MemoryScope::Project,
        "2026-02-01T00:00:00Z",
        cwd,
        &home,
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(local_dir.join("MEMORY.md")).unwrap(),
        "fresh"
    );
    assert!(
        !local_dir.join("orphan.md").exists(),
        "replace must wipe stale .md entries before copying"
    );
}

#[test]
fn test_mark_synced_records_timestamp_without_touching_md() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let home = tmp.path().join("home");

    let local_dir = agent_memory_dir("Plan", MemoryScope::Local, cwd, &home);
    fs::create_dir_all(&local_dir).unwrap();
    fs::write(local_dir.join("MEMORY.md"), "user-curated").unwrap();

    mark_snapshot_synced(
        "Plan",
        MemoryScope::Local,
        "2026-03-01T00:00:00Z",
        cwd,
        &home,
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(local_dir.join("MEMORY.md")).unwrap(),
        "user-curated",
        "mark_synced must not change .md content"
    );
    let action = check_agent_memory_snapshot("Plan", MemoryScope::Local, cwd, &home);
    assert_eq!(action, SnapshotAction::None);
}

#[test]
fn test_plugin_namespaced_agent_type_sanitized() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path();
    let dir = snapshot_dir_for_agent("plugin:agent", cwd);
    assert!(
        dir.ends_with("plugin-agent"),
        "colon must be sanitized to dash; got {dir:?}"
    );
}
