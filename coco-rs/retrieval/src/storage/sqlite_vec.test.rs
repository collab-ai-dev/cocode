use super::*;
use tempfile::TempDir;

fn test_chunk(
    id: &str,
    source_id: &str,
    filepath: &str,
    content: &str,
    content_hash: &str,
) -> CodeChunk {
    CodeChunk {
        id: id.to_string(),
        source_id: source_id.to_string(),
        filepath: filepath.to_string(),
        language: "rust".to_string(),
        content: content.to_string(),
        start_line: 1,
        end_line: 1,
        embedding: None,
        modified_time: Some(1700000000),
        workspace: source_id.to_string(),
        content_hash: content_hash.to_string(),
        indexed_at: 1700000100,
        parent_symbol: None,
        is_overview: false,
    }
}

#[tokio::test]
async fn test_open_database() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();
    assert!(store.table_exists().await.unwrap());
}

#[tokio::test]
async fn test_store_and_count() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws:test.rs:0", "ws", "test.rs", "fn main() {}", "abc123"),
        test_chunk("ws:test.rs:1", "ws", "test.rs", "fn foo() {}", "abc123"),
    ];

    store.store_chunks(&chunks).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 2);
}

#[tokio::test]
async fn test_delete_by_path() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws:a.rs:0", "ws", "a.rs", "fn a() {}", "hash_a"),
        test_chunk("ws:b.rs:0", "ws", "b.rs", "fn b() {}", "hash_b"),
    ];

    store.store_chunks(&chunks).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 2);

    let deleted = store.delete_by_path("a.rs").await.unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_get_file_metadata() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws:test.rs:0", "ws", "test.rs", "fn main() {}", "abc123"),
        test_chunk("ws:test.rs:1", "ws", "test.rs", "fn foo() {}", "abc123"),
    ];

    store.store_chunks(&chunks).await.unwrap();

    let metadata = store.get_file_metadata("ws", "test.rs").await.unwrap();
    assert!(metadata.is_some());
    let meta = metadata.unwrap();
    assert_eq!(meta.filepath, "test.rs");
    assert_eq!(meta.workspace, "ws");
    assert_eq!(meta.content_hash, "abc123");
    assert_eq!(meta.mtime, 1700000000);

    let metadata = store
        .get_file_metadata("ws", "nonexistent.rs")
        .await
        .unwrap();
    assert!(metadata.is_none());
}

#[tokio::test]
async fn test_get_workspace_files() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws:a.rs:0", "ws", "a.rs", "fn a() {}", "hash_a"),
        test_chunk("ws:a.rs:1", "ws", "a.rs", "fn a2() {}", "hash_a"),
        test_chunk("ws:b.rs:0", "ws", "b.rs", "fn b() {}", "hash_b"),
    ];

    store.store_chunks(&chunks).await.unwrap();

    let files = store.get_workspace_files("ws").await.unwrap();
    assert_eq!(files.len(), 2);

    let filepaths: Vec<_> = files.iter().map(|f| f.filepath.as_str()).collect();
    assert!(filepaths.contains(&"a.rs"));
    assert!(filepaths.contains(&"b.rs"));
}

#[tokio::test]
async fn test_delete_workspace() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws1:a.rs:0", "ws1", "a.rs", "fn a() {}", "hash_a"),
        test_chunk("ws2:b.rs:0", "ws2", "b.rs", "fn b() {}", "hash_b"),
    ];

    store.store_chunks(&chunks).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 2);

    let deleted = store.delete_workspace("ws1").await.unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(store.count().await.unwrap(), 1);

    let files = store.get_workspace_files("ws2").await.unwrap();
    assert_eq!(files.len(), 1);
}

#[tokio::test]
async fn test_fts_search_returns_empty() {
    // FTS5 has been removed; search_fts always returns empty
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![test_chunk(
        "ws:auth.rs:0",
        "ws",
        "auth.rs",
        "fn authenticate_user(username: &str) -> bool",
        "hash1",
    )];

    store.store_chunks(&chunks).await.unwrap();

    let results = store.search_fts("authenticate", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_load_all_chunk_refs() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    let chunks = vec![
        test_chunk("ws:a.rs:0", "ws", "a.rs", "fn a() {}", "hash_a"),
        test_chunk("ws:b.rs:0", "ws", "b.rs", "fn b() {}", "hash_b"),
    ];

    store.store_chunks(&chunks).await.unwrap();

    let refs = store.load_all_chunk_refs().await.unwrap();
    assert_eq!(refs.len(), 2);
    assert!(refs.contains_key("ws:a.rs:0"));
    assert!(refs.contains_key("ws:b.rs:0"));

    let a_ref = &refs["ws:a.rs:0"];
    assert_eq!(a_ref.filepath, "a.rs");
    assert_eq!(a_ref.content_hash, "hash_a");
}

#[tokio::test]
async fn test_bm25_metadata() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open(dir.path()).unwrap();

    assert!(!store.bm25_metadata_exists().await.unwrap());

    let metadata = Bm25Metadata {
        avgdl: 100.5,
        total_docs: 42,
        updated_at: 1700000000,
    };
    store.save_bm25_metadata(&metadata).await.unwrap();

    assert!(store.bm25_metadata_exists().await.unwrap());

    let loaded = store.load_bm25_metadata().await.unwrap().unwrap();
    assert!((loaded.avgdl - 100.5).abs() < f32::EPSILON);
    assert_eq!(loaded.total_docs, 42);
    assert_eq!(loaded.updated_at, 1700000000);
}

#[tokio::test]
async fn test_vector_search() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open_with_dimension(dir.path(), 4).unwrap();

    let chunks = vec![
        CodeChunk {
            id: "1".to_string(),
            source_id: "ws".to_string(),
            filepath: "a.rs".to_string(),
            language: "rust".to_string(),
            content: "fn auth() {}".to_string(),
            start_line: 1,
            end_line: 1,
            embedding: Some(vec![0.1, 0.2, 0.3, 0.4]),
            modified_time: None,
            workspace: "ws".to_string(),
            content_hash: "hash1".to_string(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        },
        CodeChunk {
            id: "2".to_string(),
            source_id: "ws".to_string(),
            filepath: "b.rs".to_string(),
            language: "rust".to_string(),
            content: "fn db() {}".to_string(),
            start_line: 1,
            end_line: 1,
            embedding: Some(vec![0.9, 0.8, 0.7, 0.6]),
            modified_time: None,
            workspace: "ws".to_string(),
            content_hash: "hash2".to_string(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        },
    ];

    store.store_chunks(&chunks).await.unwrap();

    let query = vec![0.1, 0.2, 0.3, 0.4];
    let results = store.search_vector_with_distance(&query, 2).await.unwrap();
    assert_eq!(results.len(), 2);
    // First result should be closest (id "1")
    assert_eq!(results[0].0.id, "1");
    assert!(results[0].1 < results[1].1); // closer distance
}

#[tokio::test]
async fn test_dimension_mismatch() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open_with_dimension(dir.path(), 4).unwrap();

    let wrong_dim = vec![0.1, 0.2]; // dimension 2, expected 4
    let result = store.search_vector(&wrong_dim, 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_dimension_reset_on_open() {
    let dir = TempDir::new().unwrap();

    // Open with dimension 4 and store a chunk with embedding
    {
        let store = SqliteVecStore::open_with_dimension(dir.path(), 4).unwrap();
        let chunk = CodeChunk {
            id: "1".to_string(),
            source_id: "ws".to_string(),
            filepath: "a.rs".to_string(),
            language: "rust".to_string(),
            content: "fn auth() {}".to_string(),
            start_line: 1,
            end_line: 1,
            embedding: Some(vec![0.1, 0.2, 0.3, 0.4]),
            modified_time: None,
            workspace: "ws".to_string(),
            content_hash: "hash1".to_string(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        };
        store.store_chunks(&[chunk]).await.unwrap();

        // Verify vector search works with dim=4
        let results = store
            .search_vector(&[0.1, 0.2, 0.3, 0.4], 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    // Re-open with different dimension — vector data should be cleared
    {
        let store = SqliteVecStore::open_with_dimension(dir.path(), 8).unwrap();

        // Non-vector data (code_chunks) should be preserved
        assert_eq!(store.count().await.unwrap(), 1);

        // Vector search with new dimension should return nothing (old embeddings dropped)
        let results = store
            .search_vector(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8], 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }
}

#[tokio::test]
async fn test_insert_embedding_dimension_check() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVecStore::open_with_dimension(dir.path(), 4).unwrap();

    // Storing a chunk with wrong embedding dimension should fail
    let chunk = CodeChunk {
        id: "1".to_string(),
        source_id: "ws".to_string(),
        filepath: "a.rs".to_string(),
        language: "rust".to_string(),
        content: "fn auth() {}".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: Some(vec![0.1, 0.2]), // dim 2, expected 4
        modified_time: None,
        workspace: "ws".to_string(),
        content_hash: "hash1".to_string(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };
    let result = store.store_chunks(&[chunk]).await;
    assert!(result.is_err());
}

#[test]
fn test_parse_vec0_dimension() {
    let sql = "CREATE VIRTUAL TABLE chunks_vec USING vec0(chunk_id TEXT PRIMARY KEY, embedding float[1536])";
    assert_eq!(SqliteVecStore::parse_vec0_dimension(sql), Some(1536));

    let sql2 =
        "CREATE VIRTUAL TABLE chunks_vec USING vec0(chunk_id TEXT PRIMARY KEY, embedding float[4])";
    assert_eq!(SqliteVecStore::parse_vec0_dimension(sql2), Some(4));

    assert_eq!(SqliteVecStore::parse_vec0_dimension("no match"), None);
}
