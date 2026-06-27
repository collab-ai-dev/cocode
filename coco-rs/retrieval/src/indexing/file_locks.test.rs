use super::*;

#[tokio::test]
async fn test_try_lock_success() {
    let locks = FileIndexLocks::new();
    let path = PathBuf::from("test.rs");

    let guard = locks.try_lock(&path).await;
    assert!(guard.is_some());
}

#[tokio::test]
async fn test_try_lock_conflict() {
    let locks = FileIndexLocks::new();
    let path = PathBuf::from("test.rs");

    // First lock succeeds
    let _guard1 = locks.try_lock(&path).await.unwrap();

    // Second lock fails (same file)
    let guard2 = locks.try_lock(&path).await;
    assert!(guard2.is_none());
}

#[tokio::test]
async fn test_different_files() {
    let locks = FileIndexLocks::new();
    let path1 = PathBuf::from("file1.rs");
    let path2 = PathBuf::from("file2.rs");

    // Both locks should succeed (different files)
    let guard1 = locks.try_lock(&path1).await;
    let guard2 = locks.try_lock(&path2).await;

    assert!(guard1.is_some());
    assert!(guard2.is_some());
}

#[tokio::test]
async fn test_lock_release() {
    let locks = FileIndexLocks::new();
    let path = PathBuf::from("test.rs");

    // Acquire and release lock
    {
        let _guard = locks.try_lock(&path).await.unwrap();
        // guard is dropped here
    }

    // Should be able to acquire again
    let guard = locks.try_lock(&path).await;
    assert!(guard.is_some());
}

#[tokio::test]
async fn test_cleanup() {
    let locks = FileIndexLocks::new();
    let path = PathBuf::from("test.rs");

    // Acquire and release lock
    {
        let _guard = locks.try_lock(&path).await.unwrap();
    }

    // Clean up the lock
    locks.cleanup(&path).await;

    // Lock should be removed
    assert!(locks.is_empty().await);
}

#[tokio::test]
async fn test_cleanup_while_locked() {
    let locks = FileIndexLocks::new();
    let path = PathBuf::from("test.rs");

    // Acquire lock
    let _guard = locks.try_lock(&path).await.unwrap();

    // Try to clean up (should not remove because lock is held)
    locks.cleanup(&path).await;

    // Lock should still be tracked
    assert_eq!(locks.len().await, 1);
}

#[tokio::test]
async fn test_blocking_lock() {
    let locks = Arc::new(FileIndexLocks::new());
    let path = PathBuf::from("test.rs");

    // Acquire lock in background
    let locks_clone = locks.clone();
    let path_clone = path.clone();
    let handle = tokio::spawn(async move {
        let _guard = locks_clone.lock(&path_clone).await;
        // Hold the lock briefly
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    });

    // Give the background task time to acquire the lock
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Try to acquire (should fail)
    let guard = locks.try_lock(&path).await;
    assert!(guard.is_none());

    // Wait for background task to complete
    handle.await.unwrap();

    // Now should be able to acquire
    let guard = locks.try_lock(&path).await;
    assert!(guard.is_some());
}
