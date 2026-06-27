use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_recent_files_empty_on_creation() {
    let service = RecentFilesService::default();
    assert_eq!(service.count().await, 0);
}

#[tokio::test]
async fn test_notify_file_accessed() {
    let service = RecentFilesService::default();
    let path = Path::new("src/main.rs");

    service.notify_file_accessed(path).await;

    assert!(service.is_recent_file(path).await);
    assert_eq!(service.count().await, 1);
}

#[tokio::test]
async fn test_get_recent_paths() {
    let service = RecentFilesService::default();

    service.notify_file_accessed("a.rs").await;
    service.notify_file_accessed("b.rs").await;
    service.notify_file_accessed("c.rs").await;

    let paths = service.get_recent_paths(10).await;
    assert_eq!(paths.len(), 3);
    // Most recent first
    assert_eq!(paths[0], PathBuf::from("c.rs"));
    assert_eq!(paths[1], PathBuf::from("b.rs"));
    assert_eq!(paths[2], PathBuf::from("a.rs"));
}

#[tokio::test]
async fn test_get_recent_chunks() {
    let dir = TempDir::new().unwrap();
    let service = RecentFilesService::default();

    // Create a temporary file
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").unwrap();

    service.notify_file_accessed(&file_path).await;

    let chunks = service.get_recent_chunks(100).await;
    assert!(!chunks.is_empty());
    assert!(chunks[0].content.contains("fn main()"));
}

#[tokio::test]
async fn test_remove_file() {
    let service = RecentFilesService::default();
    let path = Path::new("src/main.rs");

    service.notify_file_accessed(path).await;
    assert!(service.is_recent_file(path).await);

    service.remove_file(path).await;
    assert!(!service.is_recent_file(path).await);
}

#[tokio::test]
async fn test_clear() {
    let service = RecentFilesService::default();

    service.notify_file_accessed("a.rs").await;
    service.notify_file_accessed("b.rs").await;
    assert_eq!(service.count().await, 2);

    service.clear().await;
    assert_eq!(service.count().await, 0);
}

#[tokio::test]
async fn test_get_recent_chunks_nonexistent_file() {
    let service = RecentFilesService::default();

    // Notify with non-existent file
    let path = Path::new("/nonexistent/file.rs");
    service.notify_file_accessed(path).await;

    // File is tracked
    assert!(service.is_recent_file(path).await);

    // But get_recent_chunks returns empty (file doesn't exist)
    let chunks = service.get_recent_chunks(100).await;
    assert!(chunks.is_empty());
}

#[tokio::test]
async fn test_get_recent_search_results() {
    let dir = TempDir::new().unwrap();
    let service = RecentFilesService::default();

    // Create test files
    let file1 = dir.path().join("test1.rs");
    let file2 = dir.path().join("test2.rs");
    std::fs::write(&file1, "fn foo() {}").unwrap();
    std::fs::write(&file2, "fn bar() {}").unwrap();

    service.notify_file_accessed(&file1).await;
    service.notify_file_accessed(&file2).await;

    let results = service.get_recent_search_results(10).await;
    assert!(!results.is_empty());

    // Check score type
    for result in &results {
        assert_eq!(result.score_type, ScoreType::Recent);
    }
}
