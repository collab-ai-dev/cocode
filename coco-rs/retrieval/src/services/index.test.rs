use super::*;
use crate::config::RetrievalConfig;
use crate::context::RetrievalFeatures;
use tempfile::TempDir;

#[tokio::test]
async fn test_index_service_creation() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::with_code_search();
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let service = IndexService::new(ctx);

    // Get coordinator (lazy init)
    let coord = service.coordinator().await.unwrap();

    // Check features
    assert!(coord.features().search_enabled);
    // repomap disabled since repo_map config is None
    assert!(!coord.features().repomap_enabled);

    // Stop
    coord.stop().await;
}

#[tokio::test]
async fn test_session_start() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    // Create test files
    std::fs::write(dir.path().join("test1.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("test2.rs"), "fn foo() {}").unwrap();

    let features = RetrievalFeatures::with_code_search();
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let service = IndexService::new(ctx);

    // Start pipeline
    service.start_pipeline().await.unwrap();

    // Trigger session start
    let result = service.trigger_session_start().await.unwrap();

    // Should have scanned files
    assert!(result.file_count >= 2);
    assert!(result.index_receiver.is_some());

    // Cleanup
    service.stop_pipeline().await;
}

#[tokio::test]
async fn test_readiness() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::with_code_search();
    let ctx = Arc::new(
        RetrievalContext::new(config, features, dir.path().to_path_buf())
            .await
            .unwrap(),
    );

    let service = IndexService::new(ctx);

    // Before initialization, readiness check should work
    let readiness = service.search_readiness().await;
    assert!(readiness.is_some());

    // Cleanup
    service.stop_pipeline().await;
}
