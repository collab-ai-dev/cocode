use super::*;
use crate::config::RetrievalConfig;
use crate::context::RetrievalFeatures;
use tempfile::TempDir;

async fn create_test_services(
    dir: &TempDir,
) -> (
    Arc<RetrievalContext>,
    Arc<RecentFilesService>,
    Arc<IndexService>,
) {
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::MINIMAL;
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let recent = Arc::new(RecentFilesService::new(Arc::clone(&ctx)));
    let index = Arc::new(IndexService::new(Arc::clone(&ctx)));

    (ctx, recent, index)
}

#[tokio::test]
async fn test_search_service_creation() {
    let dir = TempDir::new().unwrap();
    let (ctx, recent, index) = create_test_services(&dir).await;

    let _service = SearchService::new(ctx, recent, index, None).await.unwrap();
}

#[tokio::test]
async fn test_search_disabled_returns_empty() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::NONE;
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let recent = Arc::new(RecentFilesService::new(Arc::clone(&ctx)));
    let index = Arc::new(IndexService::new(Arc::clone(&ctx)));

    let service = SearchService::new(ctx, recent, index, None).await.unwrap();
    // Test using new execute() API
    let results = service.execute("test query").await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_rewrite_query_disabled_returns_none() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    // code_search enabled but query_rewrite disabled
    let features = RetrievalFeatures::MINIMAL;
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let recent = Arc::new(RecentFilesService::new(Arc::clone(&ctx)));
    let index = Arc::new(IndexService::new(Arc::clone(&ctx)));

    let service = SearchService::new(ctx, recent, index, None).await.unwrap();
    assert!(service.rewrite_query("test").await.is_none());
}

#[tokio::test]
async fn test_has_vector_search() {
    let dir = TempDir::new().unwrap();
    let (ctx, recent, index) = create_test_services(&dir).await;

    let service = SearchService::new(ctx, recent, index, None).await.unwrap();
    // Vector search disabled by default (no embeddings configured)
    assert!(!service.has_vector_search());
}

// ========== SearchRequest Tests ==========

#[test]
fn test_search_request_builder() {
    let req = SearchRequest::new("test query");
    assert_eq!(req.query, "test query");
    assert!(matches!(req.mode, SearchMode::Hybrid));
    assert!(req.limit.is_none());

    let req = SearchRequest::new("test").bm25().limit(20);
    assert!(matches!(req.mode, SearchMode::Bm25));
    assert_eq!(req.limit, Some(20));

    let req = SearchRequest::new("test").vector().limit(10);
    assert!(matches!(req.mode, SearchMode::Vector));
    assert_eq!(req.limit, Some(10));
}

#[test]
fn test_search_request_from_str() {
    let req: SearchRequest = "test query".into();
    assert_eq!(req.query, "test query");
    assert!(matches!(req.mode, SearchMode::Hybrid));
}

#[test]
fn test_search_request_from_string() {
    let req: SearchRequest = String::from("test query").into();
    assert_eq!(req.query, "test query");
    assert!(matches!(req.mode, SearchMode::Hybrid));
}
