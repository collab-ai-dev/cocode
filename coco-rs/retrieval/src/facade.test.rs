use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_builder_basic() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::with_code_search();
    let facade = FacadeBuilder::new(config)
        .features(features)
        .build()
        .await
        .unwrap();

    assert!(facade.features().code_search);
    assert!(!facade.features().vector_search);
}

#[tokio::test]
async fn test_builder_with_workspace() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::with_code_search();
    let facade = FacadeBuilder::new(config)
        .features(features)
        .workspace(dir.path().to_path_buf())
        .build()
        .await
        .unwrap();

    assert_eq!(facade.workspace_root(), dir.path());
}

#[tokio::test]
async fn test_facade_search_disabled_returns_empty() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::none();
    let facade = FacadeBuilder::new(config)
        .features(features)
        .build()
        .await
        .unwrap();

    let results = facade.search("test query").await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_service_accessors_return_arc() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let features = RetrievalFeatures::with_code_search();
    let facade = FacadeBuilder::new(config)
        .features(features)
        .build()
        .await
        .unwrap();

    // Verify all service accessors work and return Arc
    let _search: Arc<SearchService> = facade.search_service();
    let _index: Arc<IndexService> = facade.index_service();
    let _recent: Arc<RecentFilesService> = facade.recent_service();
}

#[test]
fn test_is_configured_false() {
    let dir = TempDir::new().unwrap();
    // Create a config file with enabled = false
    let config_dir = dir.path().join(".coco");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("retrieval.toml"), "enabled = false").unwrap();
    let coco_home = dir.path().join(".coco");
    // Should return false when explicitly disabled
    assert!(!RetrievalFacade::is_configured(dir.path(), &coco_home));
}
