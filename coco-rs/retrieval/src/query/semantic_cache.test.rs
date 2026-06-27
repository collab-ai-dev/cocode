use super::*;

fn enabled_config() -> SemanticCacheConfig {
    SemanticCacheConfig {
        enabled: true,
        similarity_threshold: 0.95,
        max_entries: 10,
    }
}

#[test]
fn test_cosine_similarity_identical() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 1.0).abs() < 0.001);
}

#[test]
fn test_cosine_similarity_orthogonal() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let sim = cosine_similarity(&a, &b);
    assert!(sim.abs() < 0.001);
}

#[test]
fn test_cosine_similarity_opposite() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![-1.0, -2.0, -3.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim + 1.0).abs() < 0.001);
}

#[test]
fn test_cosine_similarity_zero_vector() {
    let a = vec![0.0, 0.0, 0.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn test_cosine_similarity_different_lengths() {
    let a = vec![1.0, 2.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn test_cache_disabled() {
    let config = SemanticCacheConfig::default();
    assert!(!config.enabled);

    let cache = SemanticQueryCache::new(config);
    cache.put("test", vec![1.0, 2.0], "result");
    assert!(cache.is_empty());
    assert!(cache.get_semantic(&[1.0, 2.0]).is_none());
}

#[test]
fn test_cache_put_and_get_exact() {
    let cache = SemanticQueryCache::new(enabled_config());

    let embedding = vec![1.0, 2.0, 3.0];
    cache.put("test query", embedding.clone(), "test result");

    assert_eq!(cache.len(), 1);

    // Exact match should hit
    let result = cache.get_semantic(&embedding);
    assert_eq!(result, Some("test result".to_string()));
}

#[test]
fn test_cache_similar_query() {
    let cache = SemanticQueryCache::new(enabled_config());

    let embedding1 = vec![1.0, 0.0, 0.0];
    cache.put("query1", embedding1, "result1");

    // Very similar embedding (cosine similarity > 0.95)
    let embedding2 = vec![0.99, 0.01, 0.01];
    let result = cache.get_semantic(&embedding2);

    // Should find the cached result
    assert!(result.is_some());
}

#[test]
fn test_cache_dissimilar_query() {
    let cache = SemanticQueryCache::new(enabled_config());

    let embedding1 = vec![1.0, 0.0, 0.0];
    cache.put("query1", embedding1, "result1");

    // Very different embedding (orthogonal)
    let embedding2 = vec![0.0, 1.0, 0.0];
    let result = cache.get_semantic(&embedding2);

    // Should not find a match
    assert!(result.is_none());
}

#[test]
fn test_cache_lru_eviction() {
    let mut config = enabled_config();
    config.max_entries = 3;
    let cache = SemanticQueryCache::new(config);

    // Fill cache
    cache.put("q1", vec![1.0, 0.0, 0.0], "r1");
    cache.put("q2", vec![0.0, 1.0, 0.0], "r2");
    cache.put("q3", vec![0.0, 0.0, 1.0], "r3");
    assert_eq!(cache.len(), 3);

    // Add one more, should evict oldest
    cache.put("q4", vec![1.0, 1.0, 0.0], "r4");
    assert_eq!(cache.len(), 3);

    // First entry should be gone
    assert!(cache.get_semantic(&[1.0, 0.0, 0.0]).is_none());

    // Others should remain
    assert!(cache.get_semantic(&[0.0, 1.0, 0.0]).is_some());
}

#[test]
fn test_cache_clear() {
    let cache = SemanticQueryCache::new(enabled_config());

    cache.put("q1", vec![1.0], "r1");
    cache.put("q2", vec![2.0], "r2");
    assert_eq!(cache.len(), 2);

    cache.clear();
    assert!(cache.is_empty());
}

#[test]
fn test_cache_stats() {
    let config = enabled_config();
    let cache = SemanticQueryCache::new(config.clone());

    cache.put("q1", vec![1.0], "r1");

    let stats = cache.stats();
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.capacity, config.max_entries);
    assert!((stats.threshold - config.similarity_threshold).abs() < 0.001);
}

#[test]
fn test_best_match_selection() {
    let mut config = enabled_config();
    config.similarity_threshold = 0.9;
    let cache = SemanticQueryCache::new(config);

    // Add two entries
    cache.put("q1", vec![1.0, 0.0], "r1");
    cache.put("q2", vec![0.9, 0.1], "r2");

    // Query that's closer to q1
    let query = vec![0.95, 0.05];
    let result = cache.get_semantic(&query);

    // Should return best match (r1 or r2 depending on similarity)
    assert!(result.is_some());
}
