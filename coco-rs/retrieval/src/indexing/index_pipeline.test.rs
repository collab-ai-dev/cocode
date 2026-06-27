use super::*;
use tempfile::TempDir;

async fn create_test_pipeline() -> (TempDir, IndexPipeline) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(SqliteStore::open(&db_path).unwrap());

    let config = RetrievalConfig::default();
    let strict_config = StrictModeConfig::default();

    let pipeline = IndexPipeline::new(db, config, dir.path().to_path_buf(), strict_config);

    (dir, pipeline)
}

#[tokio::test]
async fn test_pipeline_initial_state() {
    let (_dir, pipeline) = create_test_pipeline().await;
    assert!(matches!(
        pipeline.state().await,
        PipelineState::Uninitialized
    ));
    assert!(!pipeline.is_init_complete());
}

#[tokio::test]
async fn test_pipeline_building_state() {
    let (_dir, pipeline) = create_test_pipeline().await;
    let batch_id = BatchId::new();

    pipeline.mark_building(batch_id.clone()).await;

    let state = pipeline.state().await;
    assert!(matches!(state, PipelineState::Building { .. }));

    pipeline.update_progress(0.5).await;

    if let PipelineState::Building { progress, .. } = pipeline.state().await {
        assert_eq!(progress, 0.5);
    } else {
        panic!("Expected Building state");
    }
}

#[tokio::test]
async fn test_pipeline_ready_state() {
    let (_dir, pipeline) = create_test_pipeline().await;

    let stats = IndexStats {
        file_count: 10,
        chunk_count: 100,
        last_indexed: Some(chrono::Utc::now().timestamp()),
    };

    pipeline.mark_ready(stats.clone()).await;

    assert!(pipeline.is_init_complete());

    if let PipelineState::Ready { stats: s, .. } = pipeline.state().await {
        assert_eq!(s.file_count, 10);
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
        Readiness::Uninitialized
    ));

    // Building
    let batch_id = BatchId::new();
    pipeline.mark_building(batch_id).await;
    assert!(matches!(
        pipeline.readiness().await,
        Readiness::Building { .. }
    ));

    // Ready
    let stats = IndexStats {
        file_count: 5,
        chunk_count: 50,
        last_indexed: Some(chrono::Utc::now().timestamp()),
    };
    pipeline.mark_ready(stats).await;

    // Should be ready (no lag)
    assert!(matches!(
        pipeline.readiness().await,
        Readiness::Ready { .. }
    ));
    assert!(pipeline.is_ready().await);
}

#[tokio::test]
async fn test_pipeline_push_event() {
    let (_dir, pipeline) = create_test_pipeline().await;

    let seq = pipeline.assign_seq();
    let event = TrackedEvent::new(WatchEventKind::Changed, None, seq, "test-trace".to_string());

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
