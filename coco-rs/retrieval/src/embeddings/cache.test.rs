use super::*;
use tempfile::TempDir;

#[test]
fn test_put_and_get() {
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model-v1").unwrap();

    let embedding = vec![0.1, 0.2, 0.3, 0.4];
    cache.put("src/main.rs", "hash123", &embedding).unwrap();

    let retrieved = cache.get("src/main.rs", "hash123").unwrap();
    assert_eq!(retrieved.len(), 4);
    assert!((retrieved[0] - 0.1).abs() < 0.001);
}

#[test]
fn test_artifact_id_isolation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cache.db");

    // Store with model v1
    let cache_v1 = EmbeddingCache::open(&path, "model-v1").unwrap();
    cache_v1
        .put("src/lib.rs", "hash123", &[1.0, 2.0, 3.0])
        .unwrap();

    // Try to retrieve with model v2 - should not find it
    let cache_v2 = EmbeddingCache::open(&path, "model-v2").unwrap();
    assert!(cache_v2.get("src/lib.rs", "hash123").is_none());

    // Original model should still work
    assert!(cache_v1.get("src/lib.rs", "hash123").is_some());
}

#[test]
fn test_delete_by_filepath() {
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model").unwrap();

    // Store embeddings for multiple files
    cache.put("file_a.rs", "hash_a1", &[1.0, 2.0]).unwrap();
    cache.put("file_a.rs", "hash_a2", &[3.0, 4.0]).unwrap(); // same file, different hash
    cache.put("file_b.rs", "hash_b1", &[5.0, 6.0]).unwrap();

    assert_eq!(cache.count().unwrap(), 3);

    // Delete all embeddings for file_a.rs
    let deleted = cache.delete_by_filepath("file_a.rs").unwrap();
    assert_eq!(deleted, 2);

    // Verify file_a.rs entries are gone
    assert!(cache.get("file_a.rs", "hash_a1").is_none());
    assert!(cache.get("file_a.rs", "hash_a2").is_none());

    // file_b.rs should still exist
    assert!(cache.get("file_b.rs", "hash_b1").is_some());
    assert_eq!(cache.count().unwrap(), 1);
}

#[test]
fn test_batch_operations() {
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model").unwrap();

    let entries = vec![
        ("file1.rs".to_string(), "hash1".to_string(), vec![0.1, 0.2]),
        ("file2.rs".to_string(), "hash2".to_string(), vec![0.3, 0.4]),
        ("file3.rs".to_string(), "hash3".to_string(), vec![0.5, 0.6]),
    ];

    cache.put_batch(&entries).unwrap();

    let queries: Vec<(String, String)> = vec![
        ("file1.rs".to_string(), "hash1".to_string()),
        ("file2.rs".to_string(), "hash2".to_string()),
        ("missing.rs".to_string(), "missing".to_string()),
    ];
    let results = cache.get_batch(&queries);

    assert_eq!(results.len(), 2); // Only file1 and file2 found
}

#[test]
fn test_prune_stale() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cache.db");

    // Store with old model
    let cache_old = EmbeddingCache::open(&path, "model-old").unwrap();
    cache_old.put("file1.rs", "hash1", &[1.0]).unwrap();
    cache_old.put("file2.rs", "hash2", &[2.0]).unwrap();

    // Store with new model
    let cache_new = EmbeddingCache::open(&path, "model-new").unwrap();
    cache_new.put("file3.rs", "hash3", &[3.0]).unwrap();

    // Prune old entries
    let pruned = cache_new.prune_stale().unwrap();
    assert_eq!(pruned, 2);

    // Verify old entries are gone
    assert!(cache_old.get("file1.rs", "hash1").is_none());
    // New entries remain
    assert!(cache_new.get("file3.rs", "hash3").is_some());
}

#[test]
fn test_count() {
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model").unwrap();

    assert_eq!(cache.count().unwrap(), 0);

    cache.put("file1.rs", "hash1", &[1.0]).unwrap();
    cache.put("file2.rs", "hash2", &[2.0]).unwrap();

    assert_eq!(cache.count().unwrap(), 2);
}

#[test]
fn test_same_content_different_files() {
    // Test that same content in different files are stored separately
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model").unwrap();

    let same_hash = "same_content_hash";
    cache.put("file_a.rs", same_hash, &[1.0, 2.0]).unwrap();
    cache.put("file_b.rs", same_hash, &[1.0, 2.0]).unwrap();

    assert_eq!(cache.count().unwrap(), 2);

    // Delete file_a.rs should not affect file_b.rs
    cache.delete_by_filepath("file_a.rs").unwrap();

    assert!(cache.get("file_a.rs", same_hash).is_none());
    assert!(cache.get("file_b.rs", same_hash).is_some());
}

#[test]
fn test_byte_conversion() {
    let original = vec![0.1234, 5.6789, -1.0, 0.0];
    let bytes = f32_vec_to_bytes(&original);
    let converted = bytes_to_f32_vec(&bytes);

    assert_eq!(original.len(), converted.len());
    for (a, b) in original.iter().zip(converted.iter()) {
        assert!((a - b).abs() < 0.0001);
    }
}

// Bulk lookup tests (from cache_ext.rs)

fn create_test_cache() -> (TempDir, EmbeddingCache) {
    let dir = TempDir::new().unwrap();
    let cache = EmbeddingCache::open(&dir.path().join("cache.db"), "test-model").unwrap();
    (dir, cache)
}

#[test]
fn test_bulk_lookup_empty() {
    let (_dir, cache) = create_test_cache();
    let result = cache.get_batch_bulk(&[]).unwrap();
    assert!(result.all_hits()); // Empty is considered all hits
    assert_eq!(result.total(), 0);
}

#[test]
fn test_bulk_lookup_all_hits() {
    let (_dir, cache) = create_test_cache();

    // Insert some entries
    cache.put("file1.rs", "hash1", &[0.1, 0.2]).unwrap();
    cache.put("file2.rs", "hash2", &[0.3, 0.4]).unwrap();

    let entries = vec![
        ("file1.rs".to_string(), "hash1".to_string()),
        ("file2.rs".to_string(), "hash2".to_string()),
    ];

    let result = cache.get_batch_bulk(&entries).unwrap();
    assert!(result.all_hits());
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.misses.len(), 0);
    assert!((result.hit_ratio() - 1.0).abs() < 0.001);
}

#[test]
fn test_bulk_lookup_all_misses() {
    let (_dir, cache) = create_test_cache();

    let entries = vec![
        ("missing1.rs".to_string(), "hash1".to_string()),
        ("missing2.rs".to_string(), "hash2".to_string()),
    ];

    let result = cache.get_batch_bulk(&entries).unwrap();
    assert!(result.all_misses());
    assert_eq!(result.hits.len(), 0);
    assert_eq!(result.misses.len(), 2);
    assert!(result.hit_ratio() < 0.001);
}

#[test]
fn test_bulk_lookup_mixed() {
    let (_dir, cache) = create_test_cache();

    // Insert only one entry
    cache.put("found.rs", "hash1", &[0.1, 0.2]).unwrap();

    let entries = vec![
        ("found.rs".to_string(), "hash1".to_string()),
        ("missing.rs".to_string(), "hash2".to_string()),
    ];

    let result = cache.get_batch_bulk(&entries).unwrap();
    assert!(!result.all_hits());
    assert!(!result.all_misses());
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.misses.len(), 1);
    assert!((result.hit_ratio() - 0.5).abs() < 0.001);
}

#[test]
fn test_deduplicated_lookup() {
    let (_dir, cache) = create_test_cache();

    // Two files with same content hash
    let entries = vec![
        ("file_a.rs".to_string(), "same_hash".to_string()),
        ("file_b.rs".to_string(), "same_hash".to_string()),
        ("file_c.rs".to_string(), "different_hash".to_string()),
    ];

    let (hits, unique_hashes) = cache.get_batch_deduplicated(&entries).unwrap();
    assert!(hits.is_empty()); // Nothing cached
    assert_eq!(unique_hashes.len(), 2); // Only 2 unique hashes
    assert!(unique_hashes.contains(&"same_hash".to_string()));
    assert!(unique_hashes.contains(&"different_hash".to_string()));
}

#[test]
fn test_hit_ratio() {
    let result = CacheLookupResult {
        hits: vec![
            ("a".to_string(), "h1".to_string(), vec![0.1]),
            ("b".to_string(), "h2".to_string(), vec![0.2]),
        ],
        misses: vec![("c".to_string(), "h3".to_string())],
    };
    // 2 hits, 1 miss = 2/3 ≈ 0.667
    assert!((result.hit_ratio() - 0.667).abs() < 0.01);
}
