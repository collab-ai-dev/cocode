use super::*;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_cleanup_removes_files_older_than_retention() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_dir = dir.path();

    let old1 = snapshot_dir.join("snapshot-bash-100-aaa.sh");
    let old2 = snapshot_dir.join("snapshot-zsh-200-bbb.sh");
    fs::write(&old1, "# old").await.unwrap();
    fs::write(&old2, "# old").await.unwrap();

    // Zero retention → every file is stale.
    let removed = cleanup_stale_snapshots(snapshot_dir, "active", Duration::ZERO)
        .await
        .expect("cleanup");
    assert_eq!(removed, 2);
    assert!(!old1.exists());
    assert!(!old2.exists());
}

#[tokio::test]
async fn test_cleanup_preserves_fresh_files() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_dir = dir.path();

    let recent = snapshot_dir.join("snapshot-bash-123-abc.sh");
    fs::write(&recent, "# fresh").await.unwrap();

    // Long retention → file is not yet stale.
    let removed = cleanup_stale_snapshots(snapshot_dir, "active", Duration::from_secs(3600))
        .await
        .expect("cleanup");
    assert_eq!(removed, 0);
    assert!(recent.exists());
}

#[tokio::test]
async fn test_cleanup_handles_missing_dir() {
    let dir = tempdir().expect("create temp dir");
    let nonexistent = dir.path().join("nonexistent");
    let removed = cleanup_stale_snapshots(&nonexistent, "x", Duration::from_secs(0))
        .await
        .expect("cleanup");
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_cleanup_skips_non_files() {
    let dir = tempdir().expect("create temp dir");
    let snapshot_dir = dir.path();
    // A subdir must NOT be deleted.
    let subdir = snapshot_dir.join("inner");
    tokio::fs::create_dir_all(&subdir).await.unwrap();

    let removed = cleanup_stale_snapshots(snapshot_dir, "x", Duration::ZERO)
        .await
        .expect("cleanup");
    assert_eq!(removed, 0);
    assert!(subdir.exists());
}
