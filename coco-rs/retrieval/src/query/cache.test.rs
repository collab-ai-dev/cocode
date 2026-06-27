use super::*;
use crate::query::QueryIntent;
use crate::query::RewriteSource;
use tempfile::TempDir;

async fn create_test_cache() -> (RewriteCache, TempDir) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = RewriteCacheConfig {
        enabled: true,
        ttl_secs: 3600,
        max_entries: 100,
    };

    let config_hash = RewriteCache::compute_llm_config_hash("openai", "gpt-4o-mini");
    let cache = RewriteCache::new(db, config, &config_hash).await.unwrap();
    (cache, dir)
}

#[tokio::test]
async fn test_cache_put_and_get() {
    let (cache, _dir) = create_test_cache().await;

    let query = "test query";
    let result = RewrittenQuery::unchanged(query)
        .with_intent(QueryIntent::Definition)
        .with_source(RewriteSource::Rule);

    // Put into cache
    cache.put(query, &result).await.unwrap();

    // Get from cache
    let cached = cache.get(query).await.unwrap();
    assert!(cached.is_some());
    let cached = cached.unwrap();
    assert_eq!(cached.original, query);
    assert_eq!(cached.intent, QueryIntent::Definition);
}

#[tokio::test]
async fn test_cache_miss() {
    let (cache, _dir) = create_test_cache().await;

    let cached = cache.get("nonexistent").await.unwrap();
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_cache_disabled() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = RewriteCacheConfig {
        enabled: false,
        ttl_secs: 3600,
        max_entries: 100,
    };

    let config_hash = RewriteCache::compute_llm_config_hash("openai", "gpt-4o-mini");
    let cache = RewriteCache::new(db, config, &config_hash).await.unwrap();

    let result = RewrittenQuery::unchanged("test");
    cache.put("test", &result).await.unwrap();

    let cached = cache.get("test").await.unwrap();
    assert!(cached.is_none()); // Cache disabled
}

#[tokio::test]
async fn test_cache_stats() {
    let (cache, _dir) = create_test_cache().await;

    // Add some entries
    for i in 0..5 {
        let query = format!("query {i}");
        let result = RewrittenQuery::unchanged(&query);
        cache.put(&query, &result).await.unwrap();
    }

    let stats = cache.stats().await.unwrap();
    assert_eq!(stats.total_entries, 5);
    assert_eq!(stats.valid_entries, 5);
    assert_eq!(stats.expired_entries, 0);
}

#[tokio::test]
async fn test_cache_hit_count() {
    let (cache, _dir) = create_test_cache().await;

    let query = "test";
    let result = RewrittenQuery::unchanged(query);
    cache.put(query, &result).await.unwrap();

    // Get multiple times
    for _ in 0..3 {
        let _ = cache.get(query).await.unwrap();
    }

    let stats = cache.stats().await.unwrap();
    assert_eq!(stats.total_hits, 3);
}

#[tokio::test]
async fn test_cache_clear() {
    let (cache, _dir) = create_test_cache().await;

    let result = RewrittenQuery::unchanged("test");
    cache.put("test", &result).await.unwrap();

    cache.clear().await.unwrap();

    let stats = cache.stats().await.unwrap();
    assert_eq!(stats.total_entries, 0);
}
