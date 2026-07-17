use super::*;
use pretty_assertions::assert_eq;

fn memdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn test_delete_removes_file_and_index_line() {
    let dir = memdir();
    write(dir.path(), "topic-a.md", "a body");
    write(dir.path(), "topic-b.md", "b body");
    write(
        dir.path(),
        ENTRYPOINT_NAME,
        "# Memory\n\n- [Topic A](topic-a.md) — hook a\n- [Topic B](topic-b.md) — hook b\n",
    );

    delete_entry(dir.path(), "topic-a.md").unwrap();

    assert!(!dir.path().join("topic-a.md").exists());
    assert!(dir.path().join("topic-b.md").exists());
    let index = std::fs::read_to_string(dir.path().join(ENTRYPOINT_NAME)).unwrap();
    assert!(!index.contains("topic-a.md"), "dangling pointer pruned");
    assert!(index.contains("topic-b.md"), "other pointer kept");
    assert!(index.contains("# Memory"), "header preserved");
}

#[test]
fn test_delete_missing_file_still_prunes_index() {
    let dir = memdir();
    // No topic-a.md on disk, but a dangling index line points to it.
    write(
        dir.path(),
        ENTRYPOINT_NAME,
        "- [Gone](topic-a.md) — stale\n- [Keep](topic-b.md) — ok\n",
    );

    delete_entry(dir.path(), "topic-a.md").unwrap();

    let index = std::fs::read_to_string(dir.path().join(ENTRYPOINT_NAME)).unwrap();
    assert!(!index.contains("topic-a.md"));
    assert!(index.contains("topic-b.md"));
}

#[test]
fn test_delete_is_idempotent() {
    let dir = memdir();
    write(dir.path(), "topic-a.md", "a");
    write(dir.path(), ENTRYPOINT_NAME, "- [A](topic-a.md) — h\n");

    delete_entry(dir.path(), "topic-a.md").unwrap();
    // Second delete: file already gone, index already pruned → no error.
    delete_entry(dir.path(), "topic-a.md").unwrap();

    let index = std::fs::read_to_string(dir.path().join(ENTRYPOINT_NAME)).unwrap();
    assert!(!index.contains("topic-a.md"));
}

#[test]
fn test_delete_no_index_file_ok() {
    let dir = memdir();
    write(dir.path(), "topic-a.md", "a");
    // No MEMORY.md at all.
    delete_entry(dir.path(), "topic-a.md").unwrap();
    assert!(!dir.path().join("topic-a.md").exists());
}

#[test]
fn test_prune_preserves_trailing_newline() {
    let content = "- [A](a.md) — h\n- [B](b.md) — h\n";
    let pruned = prune_index_lines(content, "a.md");
    assert_eq!(pruned, "- [B](b.md) — h\n");
}
