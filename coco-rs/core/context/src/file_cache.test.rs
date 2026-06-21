use super::*;

#[test]
fn test_cache_insert_and_get() {
    let mut cache = FileReadCache::new();
    let path = PathBuf::from("/test/file.rs");
    cache.insert(path.clone(), "fn main() {}".to_string());

    let cached = cache.get(&path).unwrap();
    assert_eq!(cached.content, "fn main() {}");
    assert_eq!(cached.line_count, 1);
}

#[test]
fn test_cache_invalidate() {
    let mut cache = FileReadCache::new();
    let path = PathBuf::from("/test/file.rs");
    cache.insert(path.clone(), "content".to_string());
    assert!(cache.get(&path).is_some());

    cache.invalidate(&path);
    assert!(cache.get(&path).is_none());
}

#[test]
fn test_cache_eviction() {
    let mut cache = FileReadCache::new();
    // Insert MAX + 1 entries
    for i in 0..MAX_CACHE_ENTRIES + 1 {
        cache.insert(
            PathBuf::from(format!("/test/{i}.rs")),
            format!("content {i}"),
        );
    }
    // Should be at MAX capacity
    assert_eq!(cache.len(), MAX_CACHE_ENTRIES);
    // First entry should be evicted
    assert!(cache.get(&PathBuf::from("/test/0.rs")).is_none());
}

#[test]
fn test_cache_clear() {
    let mut cache = FileReadCache::new();
    cache.insert(PathBuf::from("/a"), "a".to_string());
    cache.insert(PathBuf::from("/b"), "b".to_string());
    assert_eq!(cache.len(), 2);

    cache.clear();
    assert!(cache.is_empty());
}
