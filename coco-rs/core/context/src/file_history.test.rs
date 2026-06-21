use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn setup() -> (TempDir, TempDir) {
    let config_home = TempDir::new().expect("create config dir");
    let work_dir = TempDir::new().expect("create work dir");
    (config_home, work_dir)
}

#[tokio::test]
async fn test_track_edit_creates_backup() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("hello.txt");
    tokio::fs::write(&file, "original").await.unwrap();

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    assert!(state.tracked_files.contains(&file));
    assert_eq!(state.snapshots.len(), 1);

    let backup = state.latest_backup(&file).unwrap();
    assert!(backup.backup_file_name.is_some());
    assert_eq!(backup.version, 1);
}

#[tokio::test]
async fn test_track_edit_nonexistent_file_records_null() {
    let (config_home, _work_dir) = setup();
    let file = PathBuf::from("/tmp/coco-test-nonexistent-12345.txt");

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    let backup = state.latest_backup(&file).unwrap();
    assert!(backup.backup_file_name.is_none());
}

#[tokio::test]
async fn test_track_edit_idempotent_same_message() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("hello.txt");
    tokio::fs::write(&file, "original").await.unwrap();

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    // Should still have only one snapshot entry for this file.
    assert_eq!(state.snapshots.len(), 1);
    assert_eq!(state.snapshots[0].tracked_file_backups.len(), 1);
}

#[tokio::test]
async fn test_make_snapshot_backs_up_tracked_files() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("data.txt");
    tokio::fs::write(&file, "v1 content").await.unwrap();

    let mut state = FileHistoryState::new();
    state.track_file(file.clone());
    state
        .make_snapshot("msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    assert_eq!(state.snapshots.len(), 1);
    let backup = state.latest_backup(&file).unwrap();
    assert!(backup.backup_file_name.is_some());
}

#[tokio::test]
async fn test_rewind_restores_file_content() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("code.rs");
    tokio::fs::write(&file, "fn original()").await.unwrap();

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    // Modify the file (simulating a tool write).
    tokio::fs::write(&file, "fn modified()").await.unwrap();

    // Rewind to msg-1.
    let changed = state
        .rewind("msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    assert_eq!(changed.len(), 1);
    let content = tokio::fs::read_to_string(&file).await.unwrap();
    assert_eq!(content, "fn original()");
}

#[tokio::test]
async fn test_rewind_deletes_file_that_didnt_exist() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("new_file.txt");

    let mut state = FileHistoryState::new();
    // Track before file exists (null backup).
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    // Create the file (simulating tool creating it).
    tokio::fs::write(&file, "created content").await.unwrap();

    // Rewind — file should be deleted.
    let changed = state
        .rewind("msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    assert_eq!(changed.len(), 1);
    assert!(!file.exists());
}

#[tokio::test]
async fn test_has_any_changes_detects_modification() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("check.txt");
    tokio::fs::write(&file, "before").await.unwrap();

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    // No changes yet.
    assert!(
        !state
            .has_any_changes("msg-1", config_home.path(), "session-1")
            .await
    );

    // Modify file.
    tokio::fs::write(&file, "after").await.unwrap();
    assert!(
        state
            .has_any_changes("msg-1", config_home.path(), "session-1")
            .await
    );
}

#[tokio::test]
async fn test_get_diff_stats() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("stats.txt");
    tokio::fs::write(&file, "line1\nline2\n").await.unwrap();

    let mut state = FileHistoryState::new();
    state
        .track_edit(&file, "msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    tokio::fs::write(&file, "line1\nline2\nline3\nline4\n")
        .await
        .unwrap();

    let stats = state
        .get_diff_stats("msg-1", config_home.path(), "session-1")
        .await
        .unwrap();

    assert_eq!(stats.files_changed.len(), 1);
    assert_eq!(stats.insertions, 2);
}

#[tokio::test]
async fn test_get_diff_stats_between_two_snapshots() {
    // computeDiffStatsBetweenMessages reads structuredPatch
    // off tool results; we read the snapshot-pair file contents.
    // Either source produces the same `+X -Y` for net edits.
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("between.txt");
    tokio::fs::write(&file, "line1\nline2\n").await.unwrap();

    let mut state = FileHistoryState::new();
    // Snapshot at msg-1 captures the pre-edit content. `track_edit`
    // does NOT push a new snapshot on subsequent calls (it just
    // updates the most-recent snapshot in place), so per-turn
    // snapshot pushes go through `make_snapshot`.
    state.track_file(file.clone());
    state
        .make_snapshot("msg-1", config_home.path(), "session-1")
        .await
        .unwrap();
    // Edit between msg-1 and msg-2.
    tokio::fs::write(&file, "line1\nline2\nline3\n")
        .await
        .unwrap();
    state
        .make_snapshot("msg-2", config_home.path(), "session-1")
        .await
        .unwrap();
    // Another edit after msg-2 (no further snapshot).
    tokio::fs::write(&file, "line1\nline2\nline3\nline4\n")
        .await
        .unwrap();

    let between = state
        .get_diff_stats_between("msg-1", Some("msg-2"), config_home.path(), "session-1")
        .await
        .unwrap();
    assert_eq!(between.files_changed.len(), 1, "one file changed");
    assert_eq!(
        between.insertions, 1,
        "exactly one line added between msg-1 and msg-2"
    );
    assert_eq!(between.deletions, 0);

    let trailing = state
        .get_diff_stats_between("msg-2", None, config_home.path(), "session-1")
        .await
        .unwrap();
    assert_eq!(trailing.files_changed.len(), 1);
    assert_eq!(trailing.insertions, 1, "one line added after msg-2 on disk");
}

#[tokio::test]
async fn test_get_diff_stats_between_unchanged_returns_empty() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("noop.txt");
    tokio::fs::write(&file, "a\nb\n").await.unwrap();

    let mut state = FileHistoryState::new();
    state.track_file(file.clone());
    state
        .make_snapshot("msg-a", config_home.path(), "session-1")
        .await
        .unwrap();
    // No edit between snapshots — make_snapshot inherits backups so
    // we can still compare snapshot-to-snapshot.
    state
        .make_snapshot("msg-b", config_home.path(), "session-1")
        .await
        .unwrap();

    let stats = state
        .get_diff_stats_between("msg-a", Some("msg-b"), config_home.path(), "session-1")
        .await
        .unwrap();
    assert!(stats.files_changed.is_empty());
    assert_eq!(stats.insertions, 0);
    assert_eq!(stats.deletions, 0);
}

#[tokio::test]
async fn test_get_diff_stats_between_missing_from_errors() {
    let (config_home, _work_dir) = setup();
    let state = FileHistoryState::new();
    let res = state
        .get_diff_stats_between("nope", None, config_home.path(), "session-1")
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn test_can_restore() {
    let state = FileHistoryState {
        snapshots: vec![FileHistorySnapshot {
            message_id: "msg-42".to_string(),
            tracked_file_backups: HashMap::new(),
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap(),
        }],
        ..Default::default()
    };
    assert!(state.can_restore("msg-42"));
    assert!(!state.can_restore("msg-99"));
}

#[tokio::test]
async fn test_snapshot_cap_enforcement() {
    let (config_home, work_dir) = setup();
    let file = work_dir.path().join("cap.txt");
    tokio::fs::write(&file, "data").await.unwrap();

    let mut state = FileHistoryState::new();
    state.track_file(file.clone());

    for i in 0..=MAX_SNAPSHOTS + 5 {
        state
            .make_snapshot(&format!("msg-{i}"), config_home.path(), "session-1")
            .await
            .unwrap();
    }

    assert!(state.snapshots.len() <= MAX_SNAPSHOTS);
}

#[tokio::test]
async fn test_copy_file_history_for_resume() {
    let config_home = TempDir::new().unwrap();
    let src_dir = backup_dir(config_home.path(), "old-session");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();
    tokio::fs::write(src_dir.join("backup1"), "data1")
        .await
        .unwrap();
    tokio::fs::write(src_dir.join("backup2"), "data2")
        .await
        .unwrap();

    let copied =
        copy_file_history_for_resume(config_home.path(), "old-session", "new-session", &[], None)
            .await
            .unwrap();

    assert_eq!(copied, 2);
    let dst_dir = backup_dir(config_home.path(), "new-session");
    assert!(dst_dir.join("backup1").exists());
    assert!(dst_dir.join("backup2").exists());
}

/// `copyFileHistoryForResume` replays the snapshot chain
/// into the new session's transcript via `recordFileHistorySnapshot`.
/// Verify that a sink passed in receives every snapshot.
#[tokio::test]
async fn test_copy_file_history_for_resume_replays_snapshots_into_new_session() {
    use std::sync::Mutex;

    let config_home = TempDir::new().unwrap();
    let src_dir = backup_dir(config_home.path(), "old-session");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();

    #[derive(Default)]
    struct CapturedSink {
        records: Mutex<Vec<(String, serde_json::Value, bool)>>,
    }
    #[async_trait::async_trait]
    impl FileHistorySnapshotSink for CapturedSink {
        async fn record(
            &self,
            message_id: &str,
            snapshot: serde_json::Value,
            is_snapshot_update: bool,
        ) {
            self.records.lock().unwrap().push((
                message_id.to_string(),
                snapshot,
                is_snapshot_update,
            ));
        }
    }

    let sink = CapturedSink::default();
    let snapshots = vec![
        FileHistorySnapshot {
            message_id: "msg-a".to_string(),
            tracked_file_backups: HashMap::new(),
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100).unwrap(),
        },
        FileHistorySnapshot {
            message_id: "msg-b".to_string(),
            tracked_file_backups: HashMap::new(),
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(200).unwrap(),
        },
    ];

    copy_file_history_for_resume(
        config_home.path(),
        "old-session",
        "new-session",
        &snapshots,
        Some(&sink),
    )
    .await
    .unwrap();

    let captured = sink.records.into_inner().unwrap();
    assert_eq!(captured.len(), 2, "every snapshot should be replayed");
    assert_eq!(captured[0].0, "msg-a");
    assert!(!captured[0].2, "replay must use isSnapshotUpdate=false");
    assert_eq!(captured[1].0, "msg-b");
    assert!(!captured[1].2, "replay must use isSnapshotUpdate=false");
}

#[tokio::test]
async fn test_restore_from_snapshots() {
    let snapshots = vec![
        FileHistorySnapshot {
            message_id: "m1".to_string(),
            tracked_file_backups: {
                let mut m = HashMap::new();
                m.insert(
                    PathBuf::from("/a.txt"),
                    FileHistoryBackup {
                        backup_file_name: Some("abc@v1".to_string()),
                        version: 1,
                        backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100)
                            .unwrap(),
                    },
                );
                m
            },
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100).unwrap(),
        },
        FileHistorySnapshot {
            message_id: "m2".to_string(),
            tracked_file_backups: {
                let mut m = HashMap::new();
                m.insert(
                    PathBuf::from("/a.txt"),
                    FileHistoryBackup {
                        backup_file_name: Some("abc@v2".to_string()),
                        version: 2,
                        backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(200)
                            .unwrap(),
                    },
                );
                m.insert(
                    PathBuf::from("/b.txt"),
                    FileHistoryBackup {
                        backup_file_name: Some("def@v1".to_string()),
                        version: 1,
                        backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(200)
                            .unwrap(),
                    },
                );
                m
            },
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(200).unwrap(),
        },
    ];

    let state = FileHistoryState::restore_from_snapshots(snapshots);
    assert_eq!(state.snapshots.len(), 2);
    assert_eq!(state.tracked_files.len(), 2);
    assert_eq!(state.snapshot_sequence, 2);
}

#[tokio::test]
async fn test_rewind_skips_invalid_backup_name_without_aborting() {
    let dir = TempDir::new().unwrap();
    let config_home = TempDir::new().unwrap();
    let good_file = dir.path().join("good.txt");
    let bad_file = dir.path().join("bad.txt");
    fs::write(&good_file, "current").await.unwrap();
    fs::write(&bad_file, "current").await.unwrap();

    let backups = backup_dir(config_home.path(), "session-1");
    fs::create_dir_all(&backups).await.unwrap();
    fs::write(backups.join("good@v1"), "restored")
        .await
        .unwrap();

    let mut state = FileHistoryState::new();
    state.tracked_files.insert(good_file.clone());
    state.tracked_files.insert(bad_file.clone());
    state.snapshots.push(FileHistorySnapshot {
        message_id: "m1".to_string(),
        tracked_file_backups: {
            let mut m = HashMap::new();
            m.insert(
                good_file.clone(),
                FileHistoryBackup {
                    backup_file_name: Some("good@v1".to_string()),
                    version: 1,
                    backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100)
                        .unwrap(),
                },
            );
            m.insert(
                bad_file.clone(),
                FileHistoryBackup {
                    backup_file_name: Some("../escape".to_string()),
                    version: 1,
                    backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100)
                        .unwrap(),
                },
            );
            m
        },
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100).unwrap(),
    });

    let changed = state
        .rewind("m1", config_home.path(), "session-1")
        .await
        .unwrap();
    assert_eq!(changed, vec![good_file.clone()]);
    assert_eq!(fs::read_to_string(&good_file).await.unwrap(), "restored");
    assert_eq!(fs::read_to_string(&bad_file).await.unwrap(), "current");
}

#[test]
fn test_file_history_snapshot_serializes_with_snake_case() {
    let snapshot = FileHistorySnapshot {
        message_id: "m1".to_string(),
        tracked_file_backups: {
            let mut m = HashMap::new();
            m.insert(
                PathBuf::from("/a.txt"),
                FileHistoryBackup {
                    backup_file_name: Some("abc@v1".to_string()),
                    version: 1,
                    backup_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100)
                        .unwrap(),
                },
            );
            m
        },
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(100).unwrap(),
    };

    let value = serde_json::to_value(snapshot).unwrap();
    // Snake_case wire — coco-rs's own format.
    assert!(value.get("message_id").is_some());
    assert!(value.get("tracked_file_backups").is_some());
    assert!(
        value
            .pointer("/tracked_file_backups/~1a.txt/backup_file_name")
            .is_some()
    );
    assert!(
        value
            .pointer("/tracked_file_backups/~1a.txt/backup_time")
            .is_some()
    );
    assert!(value.get("messageId").is_none());
    assert!(value.get("trackedFileBackups").is_none());
    // Time fields are `chrono::DateTime<Utc>` so they serialize to
    // RFC 3339 strings, not numbers — Rust-idiomatic typing.
    let timestamp = value.get("timestamp").expect("timestamp present");
    assert!(
        timestamp.is_string(),
        "timestamp must be an ISO 8601 string, got {timestamp}"
    );
    let backup_time = value
        .pointer("/tracked_file_backups/~1a.txt/backup_time")
        .expect("backup_time present");
    assert!(
        backup_time.is_string(),
        "backup_time must be an ISO 8601 string, got {backup_time}"
    );
}
