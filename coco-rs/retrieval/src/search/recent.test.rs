use super::*;
use std::thread::sleep;
use std::time::Duration;

#[test]
fn test_new_cache() {
    let cache = RecentFilesCache::new(10);
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_notify_file_accessed() {
    let mut cache = RecentFilesCache::new(10);
    let path = Path::new("src/main.rs");

    cache.notify_file_accessed(path);

    assert!(!cache.is_empty());
    assert_eq!(cache.len(), 1);
    assert!(cache.contains(path));
}

#[test]
fn test_get_recent_paths() {
    let mut cache = RecentFilesCache::new(10);

    cache.notify_file_accessed("src/main.rs");
    cache.notify_file_accessed("src/lib.rs");
    cache.notify_file_accessed("src/utils.rs");

    // Most recent first
    let paths = cache.get_recent_paths(10);
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0], PathBuf::from("src/utils.rs"));
    assert_eq!(paths[1], PathBuf::from("src/lib.rs"));
    assert_eq!(paths[2], PathBuf::from("src/main.rs"));

    // Limited
    let paths = cache.get_recent_paths(2);
    assert_eq!(paths.len(), 2);
}

#[test]
fn test_lru_eviction() {
    let mut cache = RecentFilesCache::new(2);

    cache.notify_file_accessed("a.rs");
    cache.notify_file_accessed("b.rs");
    cache.notify_file_accessed("c.rs");

    // Oldest (a.rs) should be evicted
    assert!(!cache.contains("a.rs"));
    assert!(cache.contains("b.rs"));
    assert!(cache.contains("c.rs"));
    assert_eq!(cache.len(), 2);
}

#[test]
fn test_touch_updates_lru_order() {
    let mut cache = RecentFilesCache::new(2);

    cache.notify_file_accessed("a.rs");
    cache.notify_file_accessed("b.rs");

    // Touch a.rs to make it most recent
    assert!(cache.touch("a.rs"));

    // Add c.rs - should evict b.rs (now oldest)
    cache.notify_file_accessed("c.rs");

    assert!(cache.contains("a.rs"));
    assert!(!cache.contains("b.rs"));
    assert!(cache.contains("c.rs"));
}

#[test]
fn test_touch_nonexistent() {
    let mut cache = RecentFilesCache::new(10);
    assert!(!cache.touch("nonexistent.rs"));
}

#[test]
fn test_remove() {
    let mut cache = RecentFilesCache::new(10);
    let path = "src/main.rs";

    cache.notify_file_accessed(path);
    assert!(cache.contains(path));

    let removed = cache.remove(path);
    assert!(removed);
    assert!(!cache.contains(path));
}

#[test]
fn test_remove_nonexistent() {
    let mut cache = RecentFilesCache::new(10);
    assert!(!cache.remove("nonexistent.rs"));
}

#[test]
fn test_get_recent_paths_with_age() {
    let mut cache = RecentFilesCache::new(10);

    cache.notify_file_accessed("src/main.rs");

    // Small sleep to ensure measurable age
    sleep(Duration::from_millis(10));

    let results = cache.get_recent_paths_with_age(10);
    assert_eq!(results.len(), 1);
    // Age should be 0 seconds (sub-second sleep)
    assert_eq!(results[0].1, 0);
}

#[test]
fn test_files_list() {
    let mut cache = RecentFilesCache::new(10);

    cache.notify_file_accessed("a.rs");
    cache.notify_file_accessed("b.rs");

    let files = cache.files();
    assert_eq!(files.len(), 2);
}

#[test]
fn test_clear() {
    let mut cache = RecentFilesCache::new(10);

    cache.notify_file_accessed("a.rs");
    cache.notify_file_accessed("b.rs");

    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_default() {
    let cache = RecentFilesCache::default();
    assert!(cache.is_empty());
}

#[test]
fn test_get_access_time() {
    let mut cache = RecentFilesCache::new(10);
    let path = "src/main.rs";

    cache.notify_file_accessed(path);

    let ts = cache.get_access_time(path);
    assert!(ts.is_some());

    assert!(cache.get_access_time("nonexistent.rs").is_none());
}

#[test]
fn test_mru_order() {
    let mut cache = RecentFilesCache::new(10);

    cache.notify_file_accessed("a.rs");
    cache.notify_file_accessed("b.rs");
    cache.notify_file_accessed("c.rs");

    // Most recently added (c) should come first
    let paths = cache.get_recent_paths(10);
    assert_eq!(paths[0], PathBuf::from("c.rs"));
    assert_eq!(paths[1], PathBuf::from("b.rs"));
    assert_eq!(paths[2], PathBuf::from("a.rs"));
}
