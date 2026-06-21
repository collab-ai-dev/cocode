use super::*;

#[test]
fn test_compute_hash() {
    let hash1 = ChangeDetector::compute_hash("hello world");
    let hash2 = ChangeDetector::compute_hash("hello world");
    let hash3 = ChangeDetector::compute_hash("hello world!");

    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
    assert_eq!(hash1.len(), 16); // 8 bytes = 16 hex chars
}

#[test]
fn test_compute_hash_bytes() {
    let hash = ChangeDetector::compute_hash_bytes(b"test content");
    assert_eq!(hash.len(), 16);
}

#[tokio::test]
async fn test_detect_changes() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let detector = ChangeDetector::new(store.clone());

    // Initially empty catalog
    let current = HashMap::from([
        ("file1.rs".to_string(), "hash1".to_string()),
        ("file2.rs".to_string(), "hash2".to_string()),
    ]);

    let changes = detector.detect_changes("ws", &current).await.unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.status == ChangeStatus::Added));

    // Add to catalog
    detector
        .update_catalog("ws", "file1.rs", "hash1", 1000, 5, 0)
        .await
        .unwrap();
    detector
        .update_catalog("ws", "file2.rs", "hash2", 1000, 3, 0)
        .await
        .unwrap();

    // No changes now
    let changes = detector.detect_changes("ws", &current).await.unwrap();
    assert_eq!(changes.len(), 0);

    // Modify file1
    let current_modified = HashMap::from([
        ("file1.rs".to_string(), "hash1_new".to_string()),
        ("file2.rs".to_string(), "hash2".to_string()),
    ]);

    let changes = detector
        .detect_changes("ws", &current_modified)
        .await
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].filepath, "file1.rs");
    assert_eq!(changes[0].status, ChangeStatus::Modified);

    // Delete file2 (file1 unchanged from catalog's hash1)
    let current_deleted = HashMap::from([("file1.rs".to_string(), "hash1".to_string())]);

    let changes = detector
        .detect_changes("ws", &current_deleted)
        .await
        .unwrap();
    // file1: catalog has hash1, current has hash1 -> unchanged (not in changes)
    // file2: in catalog but not in current -> deleted
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].filepath, "file2.rs");
    assert_eq!(changes[0].status, ChangeStatus::Deleted);
}
