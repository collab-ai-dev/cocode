use super::*;
use async_trait::async_trait;
use tempfile::TempDir;

#[derive(Debug)]
struct MockEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn dimension(&self) -> i32 {
        128
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1; 128])
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.1; 128]).collect())
    }
}

#[tokio::test]
async fn test_hybrid_searcher_creation() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    let searcher = HybridSearcher::new(store.clone());
    assert!(!searcher.has_vector_search());

    let provider = Arc::new(MockEmbeddingProvider);
    let searcher = HybridSearcher::with_embeddings(store, provider);
    assert!(searcher.has_vector_search());
}

#[tokio::test]
async fn test_search_empty_store() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    let searcher = HybridSearcher::new(store);
    let results = searcher.search("test", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_reranker_enabled_by_config() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    // Default: no reranker
    let searcher = HybridSearcher::new(store.clone());
    assert!(!searcher.has_reranker());

    // Enable via config
    let config = RerankerConfig::default();
    assert!(config.enabled); // Enabled by default
    let searcher = HybridSearcher::new(store.clone()).with_reranker_config(&config);
    assert!(searcher.has_reranker());

    // Disable via config
    let config = RerankerConfig {
        enabled: false,
        ..Default::default()
    };
    let searcher = HybridSearcher::new(store).with_reranker_config(&config);
    assert!(!searcher.has_reranker());
}

#[tokio::test]
async fn test_reranker_with_default() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    let searcher = HybridSearcher::new(store).with_reranker();
    assert!(searcher.has_reranker());
}

#[tokio::test]
async fn test_reranker_without() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    let searcher = HybridSearcher::new(store)
        .with_reranker()
        .without_reranker();
    assert!(!searcher.has_reranker());
}

#[tokio::test]
async fn test_with_workspace_root() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    let searcher = HybridSearcher::new(store.clone());
    assert!(searcher.workspace_root.is_none());

    let searcher = HybridSearcher::new(store).with_workspace_root(dir.path());
    assert!(searcher.workspace_root.is_some());
    assert_eq!(searcher.workspace_root.as_ref().unwrap(), dir.path());
}

#[tokio::test]
async fn test_search_hydrated_without_workspace_root() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(dir.path()).unwrap());

    // Without workspace_root, search_hydrated should work but not hydrate
    let searcher = HybridSearcher::new(store);
    let results = searcher.search_hydrated("test", 10).await.unwrap();
    assert!(results.is_empty()); // Empty store
}

#[tokio::test]
async fn test_hydrate_chunk_basic() {
    use std::io::Write;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn main() {{").unwrap();
    writeln!(file, "    println!(\"hello\");").unwrap();
    writeln!(file, "}}").unwrap();

    let store_dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(store_dir.path()).unwrap());

    let searcher = HybridSearcher::new(store).with_workspace_root(dir.path());

    // Create a chunk that matches the file
    let chunk = CodeChunk {
        id: "test:test.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "test.rs".to_string(),
        language: "rust".to_string(),
        content: "old content".to_string(), // This should be replaced
        start_line: 1,
        end_line: 3,
        embedding: None,
        modified_time: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };

    let (hydrated_chunk, is_stale) = searcher.hydrate_chunk(&chunk, dir.path()).unwrap();
    assert!(hydrated_chunk.content.contains("fn main()"));
    assert!(hydrated_chunk.content.contains("println!"));
    assert!(!hydrated_chunk.content.contains("old content"));
    // No hash in the original chunk, so is_stale should be false (treated as fresh)
    assert!(!is_stale);
}

#[tokio::test]
async fn test_hydrate_chunk_file_not_found() {
    let dir = TempDir::new().unwrap();
    let store_dir = TempDir::new().unwrap();
    let store = Arc::new(crate::storage::SqliteVecStore::open(store_dir.path()).unwrap());

    let searcher = HybridSearcher::new(store).with_workspace_root(dir.path());

    // Create a chunk referencing a non-existent file
    let chunk = CodeChunk {
        id: "test:nonexistent.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "nonexistent.rs".to_string(),
        language: "rust".to_string(),
        content: "old content".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: None,
        modified_time: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };

    let result = searcher.hydrate_chunk(&chunk, dir.path());
    assert!(result.is_err()); // Should fail for non-existent file
}

#[test]
fn test_pagerank_boost() {
    use std::collections::HashMap;

    // Create mock search results
    let results = vec![
        SearchResult {
            chunk: CodeChunk {
                id: "1".to_string(),
                source_id: "test".to_string(),
                filepath: "low_rank.rs".to_string(),
                content: "content1".to_string(),
                language: "rust".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: None,
                modified_time: None,
                workspace: "test".to_string(),
                content_hash: String::new(),
                indexed_at: 0,
                parent_symbol: None,
                is_overview: false,
            },
            score: 0.9, // Higher initial score
            score_type: ScoreType::Bm25,
            is_stale: None,
        },
        SearchResult {
            chunk: CodeChunk {
                id: "2".to_string(),
                source_id: "test".to_string(),
                filepath: "high_rank.rs".to_string(),
                content: "content2".to_string(),
                language: "rust".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: None,
                modified_time: None,
                workspace: "test".to_string(),
                content_hash: String::new(),
                indexed_at: 0,
                parent_symbol: None,
                is_overview: false,
            },
            score: 0.5, // Lower initial score
            score_type: ScoreType::Bm25,
            is_stale: None,
        },
    ];

    // PageRank: high_rank.rs is more central
    let mut file_ranks = HashMap::new();
    file_ranks.insert("high_rank.rs".to_string(), 1.0);
    file_ranks.insert("low_rank.rs".to_string(), 0.1);

    // Apply 2x boost
    let boosted = HybridSearcher::apply_pagerank_boost(results, &file_ranks, 2.0);

    // High rank file should now be first (0.5 * 2.0 = 1.0 > 0.9 * 1.1 = 0.99)
    assert_eq!(boosted.len(), 2);
    assert_eq!(boosted[0].chunk.filepath, "high_rank.rs");
    assert_eq!(boosted[1].chunk.filepath, "low_rank.rs");
}

#[test]
fn test_pagerank_boost_empty_ranks() {
    use std::collections::HashMap;

    let results = vec![SearchResult {
        chunk: CodeChunk {
            id: "1".to_string(),
            source_id: "test".to_string(),
            filepath: "test.rs".to_string(),
            content: "content".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 1,
            embedding: None,
            modified_time: None,
            workspace: "test".to_string(),
            content_hash: String::new(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        },
        score: 0.5,
        score_type: ScoreType::Bm25,
        is_stale: None,
    }];

    // Empty ranks - should return unchanged
    let empty_ranks: HashMap<String, f64> = HashMap::new();
    let boosted = HybridSearcher::apply_pagerank_boost(results.clone(), &empty_ranks, 2.0);
    assert_eq!(boosted[0].score, 0.5);

    // Boost factor <= 1.0 - should return unchanged
    let mut ranks = HashMap::new();
    ranks.insert("test.rs".to_string(), 1.0);
    let boosted = HybridSearcher::apply_pagerank_boost(results, &ranks, 1.0);
    assert_eq!(boosted[0].score, 0.5);
}
