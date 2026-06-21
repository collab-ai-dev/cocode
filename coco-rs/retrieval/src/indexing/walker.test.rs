use super::*;

#[test]
fn test_with_symlink_follow() {
    let walker = FileWalker::with_symlink_follow(Path::new("/tmp"), 10, false);
    assert!(!walker.follow_symlinks);

    let walker = FileWalker::with_symlink_follow(Path::new("/tmp"), 10, true);
    assert!(walker.follow_symlinks);
}

#[test]
fn test_walker_default_follows_symlinks() {
    let walker = FileWalker::new(Path::new("/tmp"), 10);
    assert!(walker.follow_symlinks);
}

#[test]
fn test_filter_summary() {
    let walker = FileWalker::with_filter(
        Path::new("/project"),
        10,
        &["src".to_string()],
        &["vendor".to_string()],
        &["rs".to_string()],
        &["test.rs".to_string()],
    );
    let summary = walker.filter_summary();
    assert!(summary.has_filters());
}

#[cfg(unix)]
#[test]
fn test_symlink_handling() {
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Create a real file
    let real_file = root.join("real.rs");
    std::fs::write(&real_file, "fn main() {}").unwrap();

    // Create a symlink to the file
    let link_file = root.join("link.rs");
    symlink(&real_file, &link_file).unwrap();

    // Create a broken symlink
    let broken_link = root.join("broken.rs");
    symlink(root.join("nonexistent.rs"), &broken_link).unwrap();

    let walker = FileWalker::new(root, 10);
    let files = walker.walk(root).unwrap();

    // Should find real file and valid symlink, but skip broken symlink
    // Due to deduplication, if both point to same canonical path, only one is counted
    assert!(!files.is_empty());
    assert!(files.len() <= 2);

    // Verify broken symlink is not in results
    for file in &files {
        assert!(!file.ends_with("broken.rs"));
    }
}
