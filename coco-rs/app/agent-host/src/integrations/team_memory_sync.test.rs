use super::collect_md_entries;

#[test]
fn collect_md_entries_walks_recursively_and_skips_non_md() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("MEMORY.md"), "index").unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/note.md"), "nested").unwrap();
    // Non-.md files are ignored.
    std::fs::write(root.join("ignore.txt"), "nope").unwrap();

    let mut entries = collect_md_entries(root);
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "MEMORY.md");
    assert_eq!(entries[0].content, "index");
    assert_eq!(entries[1].path, "sub/note.md");
    assert_eq!(entries[1].content, "nested");
}

#[test]
fn collect_md_entries_missing_dir_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");
    assert!(collect_md_entries(&missing).is_empty());
}
