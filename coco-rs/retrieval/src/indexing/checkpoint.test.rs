use super::*;
use tempfile::TempDir;

async fn setup_checkpoint() -> (TempDir, Checkpoint) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let checkpoint = Checkpoint::new(store);
    (dir, checkpoint)
}

#[tokio::test]
async fn test_start_and_load() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert_eq!(state.workspace, "workspace1");
    assert_eq!(state.phase, IndexPhase::Scanning);
    assert_eq!(state.total_files, 100);
    assert_eq!(state.processed_files, 0);
    assert!(state.last_file.is_none());
}

#[tokio::test]
async fn test_update_progress() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .update_progress("workspace1", "file1.rs")
        .await
        .unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert_eq!(state.processed_files, 1);
    assert_eq!(state.last_file, Some("file1.rs".to_string()));
}

#[tokio::test]
async fn test_update_progress_batch() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .update_progress_batch("workspace1", 50, "file50.rs")
        .await
        .unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert_eq!(state.processed_files, 50);
    assert_eq!(state.last_file, Some("file50.rs".to_string()));
}

#[tokio::test]
async fn test_set_phase() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .set_phase("workspace1", IndexPhase::Indexing)
        .await
        .unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert_eq!(state.phase, IndexPhase::Indexing);
}

#[tokio::test]
async fn test_complete() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint.complete("workspace1").await.unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert_eq!(state.phase, IndexPhase::Completed);
    assert!(!state.is_resumable());
}

#[tokio::test]
async fn test_is_resumable() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .update_progress_batch("workspace1", 50, "file50.rs")
        .await
        .unwrap();

    let state = checkpoint.load("workspace1").await.unwrap().unwrap();
    assert!(state.is_resumable());
    assert_eq!(state.progress_percent(), 50);
    assert_eq!(state.remaining_files(), 50);
}

#[tokio::test]
async fn test_has_resumable() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    assert!(!checkpoint.has_resumable("workspace1").await.unwrap());

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .update_progress("workspace1", "file1.rs")
        .await
        .unwrap();

    assert!(checkpoint.has_resumable("workspace1").await.unwrap());

    checkpoint.complete("workspace1").await.unwrap();
    assert!(!checkpoint.has_resumable("workspace1").await.unwrap());
}

#[tokio::test]
async fn test_clear() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint.clear("workspace1").await.unwrap();

    assert!(checkpoint.load("workspace1").await.unwrap().is_none());
}

#[tokio::test]
async fn test_list_active() {
    let (_dir, checkpoint) = setup_checkpoint().await;

    // Note: Schema allows only one checkpoint (id = 1 constraint)
    // So we test with a single workspace
    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .set_phase("workspace1", IndexPhase::Indexing)
        .await
        .unwrap();

    let active = checkpoint.list_active().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].workspace, "workspace1");
    assert_eq!(active[0].phase, IndexPhase::Indexing);
}

#[tokio::test]
async fn test_progress_percent() {
    let state = CheckpointState {
        workspace: "test".to_string(),
        phase: IndexPhase::Indexing,
        total_files: 100,
        processed_files: 33,
        last_file: None,
        started_at: 0,
        updated_at: 0,
    };

    assert_eq!(state.progress_percent(), 33);

    let empty_state = CheckpointState {
        total_files: 0,
        ..state.clone()
    };
    assert_eq!(empty_state.progress_percent(), 0);
}

#[tokio::test]
async fn test_resume_builder() {
    let (_dir, checkpoint) = setup_checkpoint().await;
    let checkpoint = Arc::new(checkpoint);

    let builder = ResumeBuilder::new(checkpoint.clone(), "workspace1");

    assert!(!builder.can_resume().await.unwrap());

    checkpoint.start("workspace1", 100).await.unwrap();
    checkpoint
        .update_progress("workspace1", "file1.rs")
        .await
        .unwrap();

    assert!(builder.can_resume().await.unwrap());
    assert_eq!(
        builder.get_skip_until().await.unwrap(),
        Some("file1.rs".to_string())
    );
}
