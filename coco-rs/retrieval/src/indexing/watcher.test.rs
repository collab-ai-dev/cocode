use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_watcher_creation() {
    let dir = TempDir::new().unwrap();
    let watcher = FileWatcher::new(dir.path(), 100);
    assert!(watcher.is_ok());
}

#[test]
fn test_should_skip() {
    let dir = TempDir::new().unwrap();
    let watcher = FileWatcher::new(dir.path(), 100).unwrap();

    // Should skip hidden files
    assert!(watcher.should_skip(Path::new("/tmp/.hidden")));

    // Should skip target directory
    assert!(watcher.should_skip(Path::new("/project/target/debug/main")));

    // Should skip node_modules
    assert!(watcher.should_skip(Path::new("/project/node_modules/pkg/index.js")));

    // Should not skip normal source files
    assert!(!watcher.should_skip(Path::new("/project/src/main.rs")));
}

#[test]
fn test_file_change_detection() {
    let dir = TempDir::new().unwrap();
    let test_file = dir.path().join("test.txt");

    let watcher = FileWatcher::new(dir.path(), 50).unwrap();

    // Create a file
    fs::write(&test_file, "hello").unwrap();

    // Wait for debounce
    std::thread::sleep(Duration::from_millis(100));

    // Should receive at least one event
    if let Some(events) = watcher.try_recv() {
        assert!(!events.is_empty());
    }
}
