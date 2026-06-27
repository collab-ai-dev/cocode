use super::*;

fn make_test_chunk(id: &str, content: &str) -> CodeChunk {
    CodeChunk {
        id: id.to_string(),
        source_id: "test".to_string(),
        filepath: "test.rs".to_string(),
        language: "rust".to_string(),
        content: content.to_string(),
        start_line: 1,
        end_line: 3,
        embedding: None,
        modified_time: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    }
}

#[tokio::test]
async fn test_index_and_search() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());
    let searcher = Bm25Searcher::new(store);

    // Index some chunks
    let chunk1 = make_test_chunk("1", "fn get_user_by_id(id: i32) -> User");
    let chunk2 = make_test_chunk("2", "fn delete_user(id: i32) -> Result<()>");
    let chunk3 = make_test_chunk("3", "struct DatabaseConnection { pool: Pool }");

    searcher.index_chunk(&chunk1).await;
    searcher.index_chunk(&chunk2).await;
    searcher.index_chunk(&chunk3).await;

    // Search
    let query = SearchQuery {
        text: "get user".to_string(),
        limit: 10,
        ..Default::default()
    };

    let results = searcher.search(&query).await.unwrap();

    // Should find results
    assert!(!results.is_empty());
    // First result should be chunk1
    assert_eq!(results[0].chunk.id, "1");
}

#[tokio::test]
async fn test_doc_count() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());
    let searcher = Bm25Searcher::new(store);

    assert_eq!(searcher.doc_count().await, 0);

    let chunk = make_test_chunk("1", "fn test() {}");
    searcher.index_chunk(&chunk).await;

    assert_eq!(searcher.doc_count().await, 1);
}
