use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_strip_snapshot_preamble_success() {
    let snapshot = "noise\n# Snapshot file\nexport PATH=/bin\n";
    let cleaned = strip_snapshot_preamble(snapshot).expect("should succeed");
    assert_eq!(cleaned, "# Snapshot file\nexport PATH=/bin\n");
}

#[test]
fn test_strip_snapshot_preamble_requires_marker() {
    let result = strip_snapshot_preamble("missing header");
    assert!(result.is_err());
}

#[test]
fn test_strip_snapshot_preamble_marker_at_start() {
    let snapshot = "# Snapshot file\nexport FOO=bar\n";
    let cleaned = strip_snapshot_preamble(snapshot).expect("should succeed");
    assert_eq!(cleaned, snapshot);
}

#[test]
fn test_snapshot_config_default() {
    let config = SnapshotConfig::default();
    assert!(
        config
            .snapshot_dir
            .to_string_lossy()
            .contains("shell-snapshots")
    );
    assert_eq!(config.timeout, Duration::from_secs(10));
    assert_eq!(config.retention, Duration::from_secs(60 * 60 * 24 * 7));
}

#[test]
fn test_snapshot_config_new() {
    let home = PathBuf::from("/home/test/.coco");
    let config = SnapshotConfig::new(&home);
    assert_eq!(config.snapshot_dir, home.join("shell-snapshots"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_write_and_validate_bash_snapshot() {
    use crate::shell_types::get_shell;

    let Some(shell) = get_shell(ShellType::Bash, None) else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("test_snapshot.sh");

    let result = write_shell_snapshot(&shell, &path, Duration::from_secs(10)).await;
    assert!(result.is_ok(), "Failed to write snapshot: {result:?}");

    let content = fs::read_to_string(&path).await.expect("read snapshot");
    assert!(content.contains("# Snapshot file"));
    assert!(content.contains("# exports"));

    let validate_result = validate_snapshot(&shell, &path, Duration::from_secs(10)).await;
    assert!(
        validate_result.is_ok(),
        "Validation failed: {validate_result:?}"
    );
}

#[tokio::test]
async fn test_shell_snapshot_try_new() {
    use crate::shell_types::default_user_shell;

    let shell = default_user_shell();
    let dir = tempfile::tempdir().expect("create temp dir");
    let config = SnapshotConfig::new(dir.path());

    let snapshot = ShellSnapshot::try_new(&config, "test-session", &shell).await;

    #[cfg(unix)]
    {
        assert!(snapshot.is_some(), "Snapshot creation failed on Unix");
        let snapshot = snapshot.expect("just checked");
        assert!(snapshot.path.exists());

        let path = snapshot.path.clone();
        drop(snapshot);
        assert!(!path.exists(), "snapshot file should be deleted after drop");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_snapshot_drop_deletes_file() {
    use crate::shell_types::default_user_shell;

    let shell = default_user_shell();
    let dir = tempfile::tempdir().expect("create temp dir");
    let config = SnapshotConfig::new(dir.path());

    let snapshot = ShellSnapshot::try_new(&config, "drop-test", &shell)
        .await
        .expect("snapshot should be created");

    let path = snapshot.path.clone();
    assert!(path.exists(), "snapshot file should exist before drop");

    drop(snapshot);

    assert!(!path.exists(), "snapshot file should be deleted after drop");
}

#[cfg(unix)]
#[tokio::test]
async fn test_validate_snapshot_rejects_invalid() {
    use crate::shell_types::default_user_shell;

    let shell = default_user_shell();
    let dir = tempfile::tempdir().expect("create temp dir");
    let invalid_snapshot = dir.path().join("invalid.sh");

    fs::write(&invalid_snapshot, "exit 1")
        .await
        .expect("write invalid snapshot");

    let result = validate_snapshot(&shell, &invalid_snapshot, Duration::from_secs(5)).await;

    assert!(
        result.is_err(),
        "validation should fail for invalid snapshot"
    );
}
