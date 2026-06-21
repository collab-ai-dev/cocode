use super::*;
use crate::config::RewriteFeatures;

#[tokio::test]
async fn test_minimal_service() {
    let service = QueryRewriteService::minimal();

    let result = service.rewrite("test function").await.unwrap();
    assert_eq!(result.original, "test function");
    // Should use rule-based since LLM is not available
    assert_eq!(result.source, RewriteSource::Rule);
}

#[tokio::test]
async fn test_disabled_service() {
    let config = QueryRewriteConfig {
        enabled: false,
        ..Default::default()
    };

    let service = QueryRewriteService::new(config, None).await.unwrap();
    let result = service.rewrite("test").await.unwrap();

    assert_eq!(result.original, "test");
    assert_eq!(result.rewritten, "test");
}

#[tokio::test]
async fn test_with_expansion() {
    let config = QueryRewriteConfig {
        features: RewriteFeatures {
            expansion: true,
            case_variants: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let service = QueryRewriteService::new(config, None).await.unwrap();
    let result = service.rewrite("find function handler").await.unwrap();

    // Should have expansions for "function"
    assert!(result.has_expansion("fn") || result.has_expansion("method"));
}

#[tokio::test]
async fn test_custom_synonyms() {
    let mut config = QueryRewriteConfig::default();
    config.features.expansion = true;
    config.rules.synonyms.insert(
        "widget".to_string(),
        vec!["component".to_string(), "element".to_string()],
    );

    let service = QueryRewriteService::new(config, None).await.unwrap();
    let result = service.rewrite("find widget").await.unwrap();

    assert!(result.has_expansion("component") || result.has_expansion("element"));
}

#[tokio::test]
async fn test_with_cache() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = QueryRewriteConfig::default();
    let service = QueryRewriteService::new(config, Some(db)).await.unwrap();

    // First call
    let result1 = service.rewrite("test query").await.unwrap();
    assert_eq!(result1.source, RewriteSource::Rule);

    // Second call should hit cache
    let result2 = service.rewrite("test query").await.unwrap();
    // Note: cache source is set when retrieved
    assert_eq!(result2.source, RewriteSource::Cache);

    // Check cache stats
    let stats = service.cache_stats().await.unwrap();
    assert_eq!(stats.total_entries, 1);
    assert!(stats.total_hits >= 1);
}
