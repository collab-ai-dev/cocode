use super::*;
use tempfile::TempDir;

async fn create_test_coordinator() -> (TempDir, Arc<IndexCoordinator>) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = RetrievalConfig::default();
    let coord = Arc::new(IndexCoordinator::new(
        config,
        "test".to_string(),
        dir.path().to_path_buf(),
        store,
    ));

    (dir, coord)
}

#[tokio::test]
async fn test_initial_state() {
    let (_dir, coord) = create_test_coordinator().await;
    assert_eq!(coord.state().await, IndexState::Uninitialized);
    assert_eq!(coord.epoch(), 0);
}

#[tokio::test]
async fn test_mark_building() {
    let (_dir, coord) = create_test_coordinator().await;

    coord.mark_building_started().await;

    let state = coord.state().await;
    assert!(matches!(state, IndexState::Building { progress: 0.0, .. }));
    assert_eq!(coord.epoch(), 1);
}

#[tokio::test]
async fn test_mark_complete() {
    let (_dir, coord) = create_test_coordinator().await;

    coord.mark_building_started().await;
    coord
        .mark_building_complete(IndexStats {
            file_count: 10,
            chunk_count: 100,
            last_indexed: Some(12345),
        })
        .await;

    let state = coord.state().await;
    assert!(matches!(state, IndexState::Ready { .. }));
    assert!(coord.is_ready().await);
    assert_eq!(coord.epoch(), 2);
}

#[tokio::test]
async fn test_mark_failed() {
    let (_dir, coord) = create_test_coordinator().await;

    coord.mark_building_started().await;
    coord.mark_building_failed("Test error".to_string()).await;

    let state = coord.state().await;
    match state {
        IndexState::Failed { error, .. } => {
            assert_eq!(error, "Test error");
        }
        _ => panic!("Expected Failed state"),
    }
}

#[tokio::test]
async fn test_update_progress() {
    let (_dir, coord) = create_test_coordinator().await;

    coord.mark_building_started().await;
    coord.update_building_progress(0.5).await;

    let state = coord.state().await;
    match state {
        IndexState::Building { progress, .. } => {
            assert!((progress - 0.5).abs() < f32::EPSILON);
        }
        _ => panic!("Expected Building state"),
    }
}

#[tokio::test]
async fn test_freshness_check_empty_index() {
    let (dir, coord) = create_test_coordinator().await;

    // Create a test file
    std::fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

    let result = coord.check_freshness().await.unwrap();

    match result {
        FreshnessResult::Stale { changes } => {
            assert!(!changes.is_empty());
            // The file should be detected as changed
            assert!(changes.iter().any(|c| c.path().ends_with("test.rs")));
        }
        _ => panic!("Expected Stale result"),
    }
}

#[tokio::test]
async fn test_event_queue_integration() {
    let (_dir, coord) = create_test_coordinator().await;

    let queue = coord.event_queue();
    queue
        .push_simple(PathBuf::from("test.rs"), WatchEventKind::Changed)
        .await;

    assert_eq!(queue.len().await, 1);
}

#[tokio::test]
async fn test_stop() {
    let (_dir, coord) = create_test_coordinator().await;

    assert!(!coord.is_stopped());
    coord.stop();
    assert!(coord.is_stopped());
}
