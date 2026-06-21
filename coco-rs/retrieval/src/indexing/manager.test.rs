use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

use async_trait::async_trait;

/// Mock embedding provider for testing.
#[derive(Debug)]
struct MockEmbeddingProvider {
    call_count: AtomicUsize,
    dimension: i32,
}

impl MockEmbeddingProvider {
    fn new(dimension: i32) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            dimension,
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn reset_count(&self) {
        self.call_count.store(0, Ordering::SeqCst);
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn dimension(&self) -> i32 {
        self.dimension
    }

    async fn embed(&self, _text: &str) -> crate::error::Result<Vec<f32>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0.1; self.dimension as usize])
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.call_count.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(texts
            .iter()
            .map(|_| vec![0.1; self.dimension as usize])
            .collect())
    }
}

#[tokio::test]
async fn test_index_manager_new() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let config = RetrievalConfig::default();
    let _manager = IndexManager::new(config, store);
}

#[tokio::test]
async fn test_index_manager_with_embeddings() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let vec_store_path = dir.path().join("vec_store");
    let cache_path = dir.path().join("cache.db");

    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let vector_store: Arc<dyn VectorStore> =
        Arc::new(crate::storage::SqliteVecStore::open(&vec_store_path).unwrap());
    let provider = Arc::new(MockEmbeddingProvider::new(4));

    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let manager = IndexManager::with_embeddings(
        config,
        store,
        vector_store,
        provider,
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    assert!(manager.has_embeddings());
}

#[tokio::test]
async fn test_index_stores_chunks_to_vector_store() {
    let dir = TempDir::new().unwrap();
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    // Create test file
    let test_file = workspace_dir.join("test.rs");
    std::fs::write(&test_file, "fn main() {\n    println!(\"hello\");\n}").unwrap();

    let db_path = dir.path().join("test.db");
    let vec_store_path = dir.path().join("vec_store");
    let cache_path = dir.path().join("cache.db");

    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let vector_store: Arc<dyn VectorStore> =
        Arc::new(crate::storage::SqliteVecStore::open(&vec_store_path).unwrap());
    let provider = Arc::new(MockEmbeddingProvider::new(1536));

    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let mut manager = IndexManager::with_embeddings(
        config,
        store,
        vector_store.clone(),
        provider.clone(),
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    // Index the workspace
    let mut rx = manager
        .index_workspace("test", &workspace_dir)
        .await
        .unwrap();

    // Wait for indexing to complete
    while let Some(progress) = rx.recv().await {
        if matches!(
            progress.status,
            crate::indexing::progress::IndexStatus::Done
                | crate::indexing::progress::IndexStatus::Failed
        ) {
            break;
        }
    }

    // Verify chunks were stored in vector store
    let count = vector_store.count().await.unwrap();
    assert!(count > 0, "Expected chunks in vector store, got {count}");

    // Verify provider was called
    assert!(
        provider.call_count() > 0,
        "Expected provider to be called, but call_count is 0"
    );
}

#[tokio::test]
async fn test_cache_hit_skips_api_call() {
    let dir = TempDir::new().unwrap();
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    // Create test file
    let test_file = workspace_dir.join("test.rs");
    std::fs::write(&test_file, "fn foo() {}").unwrap();

    let db_path = dir.path().join("test.db");
    let vec_store_path = dir.path().join("vec_store");
    let cache_path = dir.path().join("cache.db");

    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let vector_store: Arc<dyn VectorStore> =
        Arc::new(crate::storage::SqliteVecStore::open(&vec_store_path).unwrap());
    let provider = Arc::new(MockEmbeddingProvider::new(1536));

    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    // First indexing
    let mut manager = IndexManager::with_embeddings(
        config.clone(),
        store.clone(),
        vector_store.clone(),
        provider.clone(),
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    let mut rx = manager
        .index_workspace("test", &workspace_dir)
        .await
        .unwrap();
    while let Some(progress) = rx.recv().await {
        if matches!(
            progress.status,
            crate::indexing::progress::IndexStatus::Done
                | crate::indexing::progress::IndexStatus::Failed
        ) {
            break;
        }
    }

    let first_call_count = provider.call_count();
    assert!(first_call_count > 0, "First indexing should call provider");

    // Reset provider count
    provider.reset_count();

    // Modify file slightly (add whitespace at end) - but same chunk content
    // Note: This tests that unchanged chunks reuse cache
    // For this test, we keep file exactly the same to trigger cache hit
    // Touch the file to update mtime (simulates re-indexing)
    // Actually, let's create a new manager and re-index

    // Second indexing with same content
    let store2 = Arc::new(SqliteStore::open(&db_path).unwrap());
    let mut manager2 = IndexManager::with_embeddings(
        config,
        store2,
        vector_store.clone(),
        provider.clone(),
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    // Delete catalog entries to force re-processing
    manager2.clean("test").await.unwrap();

    let mut rx = manager2
        .index_workspace("test", &workspace_dir)
        .await
        .unwrap();
    while let Some(progress) = rx.recv().await {
        if matches!(
            progress.status,
            crate::indexing::progress::IndexStatus::Done
                | crate::indexing::progress::IndexStatus::Failed
        ) {
            break;
        }
    }

    // Provider should NOT be called because cache has the embeddings
    assert_eq!(
        provider.call_count(),
        0,
        "Second indexing should use cache, but provider was called {} times",
        provider.call_count()
    );
}

#[tokio::test]
async fn test_file_deletion_clears_vector_store_and_cache() {
    let dir = TempDir::new().unwrap();
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    // Create test file
    let test_file = workspace_dir.join("test.rs");
    std::fs::write(&test_file, "fn main() {}").unwrap();

    let db_path = dir.path().join("test.db");
    let vec_store_path = dir.path().join("vec_store");
    let cache_path = dir.path().join("cache.db");

    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let vector_store: Arc<dyn VectorStore> =
        Arc::new(crate::storage::SqliteVecStore::open(&vec_store_path).unwrap());
    let provider = Arc::new(MockEmbeddingProvider::new(1536));

    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let mut manager = IndexManager::with_embeddings(
        config.clone(),
        store.clone(),
        vector_store.clone(),
        provider.clone(),
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    // First indexing
    let mut rx = manager
        .index_workspace("test", &workspace_dir)
        .await
        .unwrap();
    while let Some(progress) = rx.recv().await {
        if matches!(
            progress.status,
            crate::indexing::progress::IndexStatus::Done
                | crate::indexing::progress::IndexStatus::Failed
        ) {
            break;
        }
    }

    // Verify data exists
    let count_before = vector_store.count().await.unwrap();
    assert!(count_before > 0, "Expected chunks before deletion");

    // Delete the file
    std::fs::remove_file(&test_file).unwrap();

    // Re-index to detect deletion
    let store2 = Arc::new(SqliteStore::open(&db_path).unwrap());
    let mut manager2 = IndexManager::with_embeddings(
        config,
        store2,
        vector_store.clone(),
        provider.clone(),
        &cache_path,
        "test-model-v1",
    )
    .unwrap();

    let mut rx = manager2
        .index_workspace("test", &workspace_dir)
        .await
        .unwrap();
    while let Some(progress) = rx.recv().await {
        if matches!(
            progress.status,
            crate::indexing::progress::IndexStatus::Done
                | crate::indexing::progress::IndexStatus::Failed
        ) {
            break;
        }
    }

    // Verify vector store is empty
    let count_after = vector_store.count().await.unwrap();
    assert_eq!(count_after, 0, "Expected no chunks after file deletion");
}
