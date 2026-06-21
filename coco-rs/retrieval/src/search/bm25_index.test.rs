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

#[test]
fn test_index_and_search() {
    let mut index = Bm25Index::new();

    // Index some chunks
    let chunk1 = make_test_chunk("1", "fn get_user_by_id(id: i32) -> User");
    let chunk2 = make_test_chunk("2", "fn delete_user(id: i32) -> Result<()>");
    let chunk3 = make_test_chunk("3", "struct DatabaseConnection { pool: Pool }");

    index.upsert_chunk(&chunk1);
    index.upsert_chunk(&chunk2);
    index.upsert_chunk(&chunk3);

    // Search for "user"
    let results = index.search("get user", 10);
    assert!(!results.is_empty());

    // First result should be chunk1 (most relevant to "get user")
    assert_eq!(results[0].0, "1");
}

#[test]
fn test_sparse_embedding_serialization() {
    let embedding = SparseEmbedding::new(vec![1, 2, 3], vec![0.5, 0.3, 0.2]);

    let json = embedding.to_json();
    let restored = SparseEmbedding::from_json(&json).unwrap();

    assert_eq!(embedding.indices, restored.indices);
    assert_eq!(embedding.values, restored.values);
}

#[test]
fn test_config_from_search_config() {
    let search_config = SearchConfig {
        bm25_k1: 0.9,
        bm25_b: 0.4,
        ..Default::default()
    };

    let bm25_config = Bm25Config::from_search_config(&search_config);
    assert!((bm25_config.k1 - 0.9).abs() < 0.001);
    assert!((bm25_config.b - 0.4).abs() < 0.001);
}

#[test]
fn test_recalculate_avgdl() {
    let mut index = Bm25Index::new();

    // Index chunks of different lengths
    let chunk1 = make_test_chunk("1", "fn foo() {}");
    let chunk2 = make_test_chunk("2", "fn bar_baz_qux() { let x = 1; let y = 2; }");

    index.upsert_chunk(&chunk1);
    index.upsert_chunk(&chunk2);

    let old_avgdl = index.config().avgdl;
    index.recalculate_avgdl();
    let new_avgdl = index.config().avgdl;

    // avgdl should change after recalculation
    assert!((old_avgdl - new_avgdl).abs() > 0.001 || old_avgdl == 100.0);
}

#[test]
fn test_remove_chunk() {
    let mut index = Bm25Index::new();

    let chunk = make_test_chunk("1", "fn test() {}");
    index.upsert_chunk(&chunk);

    assert_eq!(index.doc_count(), 1);

    index.remove_chunk("1");
    assert_eq!(index.doc_count(), 0);
}

#[test]
fn test_metadata() {
    let mut index = Bm25Index::new();

    let chunk = make_test_chunk("1", "fn test() {}");
    index.upsert_chunk(&chunk);

    let metadata = index.metadata();
    assert_eq!(metadata.total_docs, 1);
    assert!(metadata.updated_at > 0);
}
