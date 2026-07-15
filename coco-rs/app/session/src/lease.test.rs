use super::*;
use crate::storage::TranscriptStore;
use crate::store::InMemoryStore;
use coco_paths::ProjectPaths;
use pretty_assertions::assert_eq;
use std::sync::Arc;

fn disk_store(dir: &std::path::Path) -> TranscriptStore {
    let paths = ProjectPaths::new(dir.to_path_buf(), &dir.join("proj"));
    TranscriptStore::new(Arc::new(paths))
}

#[test]
fn test_memory_lease_is_exclusive_within_process() {
    // The process-local registry is a process-global static (by design), so each
    // test uses distinct session ids to stay independent under parallel runs.
    let store = InMemoryStore::new();
    let lease = store
        .require_write_lease("mem-exclusive")
        .expect("first lease");
    let err = store.require_write_lease("mem-exclusive").unwrap_err();
    assert!(matches!(err, SessionLeaseError::InUse { .. }));
    assert_eq!(err.code(), SESSION_IN_USE);
    drop(lease);
    // Releasing the first lease lets the next acquire succeed.
    let _reacquired = store
        .require_write_lease("mem-exclusive")
        .expect("reacquire after drop");
}

#[test]
fn test_memory_lease_distinct_sessions_do_not_conflict() {
    let store = InMemoryStore::new();
    let _a = store
        .require_write_lease("mem-distinct-a")
        .expect("lease a");
    let _b = store
        .require_write_lease("mem-distinct-b")
        .expect("lease b");
}

#[test]
fn test_file_lease_acquires_and_writes_owner_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let store = disk_store(dir.path());
    let lease = store.require_write_lease("sess-1").expect("file lease");
    assert_eq!(lease.session_id(), "sess-1");
    assert_eq!(lease.owner().pid, std::process::id());

    // The lock file exists and carries readable owner diagnostics.
    let lock_path = store.project_paths().session_lock_path("sess-1");
    assert!(lock_path.exists());
    let bytes = std::fs::read(&lock_path).unwrap();
    let owner: LeaseOwner = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(owner.pid, std::process::id());
}

#[test]
fn test_file_lease_same_session_conflicts_and_releases_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let store = disk_store(dir.path());
    let lease = store
        .require_write_lease("sess-1")
        .expect("first file lease");
    let err = store.require_write_lease("sess-1").unwrap_err();
    assert!(matches!(err, SessionLeaseError::InUse { .. }));
    drop(lease);
    let _reacquired = store
        .require_write_lease("sess-1")
        .expect("reacquire after drop");

    // The lock file is intentionally retained across release.
    assert!(store.project_paths().session_lock_path("sess-1").exists());
}

#[test]
fn test_os_lock_contention_reports_in_use() {
    use fs2::FileExt;
    let dir = tempfile::tempdir().unwrap();
    let store = disk_store(dir.path());
    let lock_path = store.project_paths().session_lock_path("sess-x");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();

    // Hold the OS advisory lock via an independent fd (no process-local slot),
    // so acquisition must fail on the kernel lock, not the registry.
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    holder.lock_exclusive().unwrap();

    let err = acquire_file_lease(&lock_path, "sess-x").unwrap_err();
    assert!(matches!(err, SessionLeaseError::InUse { .. }));

    FileExt::unlock(&holder).unwrap();
}

#[test]
fn test_error_code_mapping() {
    let in_use = SessionLeaseError::InUse {
        session_id: "s".to_string(),
        owner: None,
    };
    assert_eq!(in_use.code(), SESSION_IN_USE);
    let unsupported = SessionLeaseError::Unsupported {
        reason: "nfs".to_string(),
    };
    assert_eq!(unsupported.code(), SESSION_LOCK_UNSUPPORTED);
}
