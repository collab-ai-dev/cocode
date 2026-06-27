use super::*;
use crate::storage::SqliteStore;
use tempfile::TempDir;

async fn create_test_pipeline() -> (TempDir, TagPipeline) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());
    let cache = Arc::new(RepoMapCache::new(db));

    let strict_config = TagStrictModeConfig::default();

    let pipeline = TagPipeline::new(
        cache,
        dir.path().to_path_buf(),
        strict_config,
        2, // worker count
    );

    (dir, pipeline)
}

#[tokio::test]
async fn test_pipeline_initial_state() {
    let (_dir, pipeline) = create_test_pipeline().await;
    assert!(matches!(
        pipeline.state().await,
        TagPipelineState::Uninitialized
    ));
    assert!(!pipeline.is_init_complete());
}

#[tokio::test]
async fn test_pipeline_building_state() {
    let (_dir, pipeline) = create_test_pipeline().await;
    let batch_id = BatchId::new();

    pipeline.mark_building(batch_id.clone()).await;

    let state = pipeline.state().await;
    assert!(matches!(state, TagPipelineState::Building { .. }));

    pipeline.update_progress(0.5).await;

    if let TagPipelineState::Building { progress, .. } = pipeline.state().await {
        assert_eq!(progress, 0.5);
    } else {
        panic!("Expected Building state");
    }
}

#[tokio::test]
async fn test_pipeline_ready_state() {
    let (_dir, pipeline) = create_test_pipeline().await;

    let stats = TagStats {
        file_count: 10,
        tag_count: 100,
        last_extracted: Some(chrono::Utc::now().timestamp()),
    };

    pipeline.mark_ready(stats.clone()).await;

    assert!(pipeline.is_init_complete());

    if let TagPipelineState::Ready { stats: s, .. } = pipeline.state().await {
        assert_eq!(s.file_count, 10);
        assert_eq!(s.tag_count, 100);
    } else {
        panic!("Expected Ready state");
    }
}

#[tokio::test]
async fn test_pipeline_readiness() {
    let (_dir, pipeline) = create_test_pipeline().await;

    // Initially uninitialized
    assert!(matches!(
        pipeline.readiness().await,
        TagReadiness::Uninitialized
    ));

    // Building
    let batch_id = BatchId::new();
    pipeline.mark_building(batch_id).await;
    assert!(matches!(
        pipeline.readiness().await,
        TagReadiness::Building { .. }
    ));

    // Ready
    let stats = TagStats {
        file_count: 5,
        tag_count: 50,
        last_extracted: Some(chrono::Utc::now().timestamp()),
    };
    pipeline.mark_ready(stats).await;

    // Should be ready (no lag)
    assert!(matches!(
        pipeline.readiness().await,
        TagReadiness::Ready { .. }
    ));
    assert!(pipeline.is_ready().await);
}

#[tokio::test]
async fn test_pipeline_push_event() {
    let (_dir, pipeline) = create_test_pipeline().await;

    let seq = pipeline.assign_seq();
    let event = TrackedEvent::new(TagEventKind::Changed, None, seq, "test-trace".to_string());

    pipeline.push_event(PathBuf::from("test.rs"), event).await;

    assert_eq!(pipeline.event_queue().len().await, 1);
}

#[tokio::test]
async fn test_pipeline_lag_tracking() {
    let (_dir, pipeline) = create_test_pipeline().await;

    // Assign some sequences
    let _seq1 = pipeline.assign_seq();
    let _seq2 = pipeline.assign_seq();

    // Initial lag should be 2
    assert_eq!(pipeline.current_lag(), 2);

    let info = pipeline.lag_info().await;
    assert_eq!(info.lag, 2);
}

#[tokio::test]
async fn test_pipeline_stop() {
    let (_dir, pipeline) = create_test_pipeline().await;

    assert!(!pipeline.is_stopped());
    pipeline.stop().await;
    assert!(pipeline.is_stopped());
}
