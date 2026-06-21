use super::*;
use tempfile::TempDir;

async fn create_test_coordinator(features: FeatureFlags) -> (TempDir, UnifiedCoordinator) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = RetrievalConfig::default();

    let coordinator =
        UnifiedCoordinator::new(config, features, dir.path().to_path_buf(), db).unwrap();

    (dir, coordinator)
}

#[tokio::test]
async fn test_coordinator_creation_both_enabled() {
    let features = FeatureFlags {
        search_enabled: true,
        repomap_enabled: true,
    };
    let (_dir, coord) = create_test_coordinator(features).await;

    assert!(coord.index_pipeline().is_some());
    assert!(coord.tag_pipeline().is_some());
    assert!(matches!(coord.state().await, UnifiedState::Uninitialized));
}

#[tokio::test]
async fn test_coordinator_creation_search_only() {
    let features = FeatureFlags {
        search_enabled: true,
        repomap_enabled: false,
    };
    let (_dir, coord) = create_test_coordinator(features).await;

    assert!(coord.index_pipeline().is_some());
    assert!(coord.tag_pipeline().is_none());
}

#[tokio::test]
async fn test_coordinator_creation_repomap_only() {
    let features = FeatureFlags {
        search_enabled: false,
        repomap_enabled: true,
    };
    let (_dir, coord) = create_test_coordinator(features).await;

    assert!(coord.index_pipeline().is_none());
    assert!(coord.tag_pipeline().is_some());
}

#[tokio::test]
async fn test_coordinator_session_start() {
    let features = FeatureFlags::default();
    let (dir, coord) = create_test_coordinator(features).await;

    // Create some test files
    std::fs::write(dir.path().join("test1.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("test2.rs"), "fn foo() {}").unwrap();

    // Start workers
    coord.start_workers().await;

    // Trigger session start
    let result = coord.trigger_session_start().await.unwrap();

    assert!(result.index_receiver.is_some());
    assert!(result.tag_receiver.is_some());
    assert!(result.file_count >= 2);

    // State should be building
    assert!(matches!(coord.state().await, UnifiedState::Building { .. }));
}

#[tokio::test]
async fn test_coordinator_stop() {
    let features = FeatureFlags::default();
    let (_dir, coord) = create_test_coordinator(features).await;

    assert!(!coord.is_stopped());
    coord.stop().await;
    assert!(coord.is_stopped());
}

#[tokio::test]
async fn test_coordinator_epoch() {
    let features = FeatureFlags::default();
    let (_dir, coord) = create_test_coordinator(features).await;

    let e1 = coord.epoch();
    let e2 = coord.next_epoch();
    let e3 = coord.epoch();

    assert_eq!(e2, e1 + 1);
    assert_eq!(e3, e2);
}

#[tokio::test]
async fn test_generate_trace_id() {
    let id1 = generate_trace_id(TriggerSource::SessionStart, 1);
    let id2 = generate_trace_id(TriggerSource::Timer, 2);
    let id3 = generate_trace_id(TriggerSource::Watcher, 3);

    assert!(id1.starts_with("session-1-"));
    assert!(id2.starts_with("timer-2-"));
    assert!(id3.starts_with("watch-3-"));
}
