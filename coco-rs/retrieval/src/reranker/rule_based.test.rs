use super::*;
use crate::types::CodeChunk;
use crate::types::ScoreType;

fn make_result(
    id: &str,
    filepath: &str,
    content: &str,
    score: f32,
    modified_time: Option<i64>,
) -> SearchResult {
    SearchResult {
        chunk: CodeChunk {
            id: id.to_string(),
            source_id: "test".to_string(),
            filepath: filepath.to_string(),
            language: "rust".to_string(),
            content: content.to_string(),
            start_line: 1,
            end_line: 10,
            embedding: None,
            modified_time,
            workspace: "test".to_string(),
            content_hash: String::new(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        },
        score,
        score_type: ScoreType::Hybrid,
        is_stale: None,
    }
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn test_default_config() {
    let config = RuleBasedRerankerConfig::default();
    assert_eq!(config.exact_match_boost, 2.0);
    assert_eq!(config.path_relevance_boost, 1.5);
    assert_eq!(config.recency_boost, 1.2);
    assert_eq!(config.recency_days_threshold, 7);
}

#[tokio::test]
async fn test_rerank_exact_match_boost() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![
        make_result("1", "src/foo.rs", "fn bar() {}", 1.0, None),
        make_result("2", "src/other.rs", "fn test_foo() {}", 1.0, None),
    ];

    reranker.rerank("foo", &mut results).await.unwrap();

    // "foo" appears in filepath of result 1 and content of result 2
    // Result 2 has "foo" in content (exact match)
    assert!(
        results[0].chunk.id == "2",
        "Result with exact match should be first"
    );
}

#[tokio::test]
async fn test_rerank_path_relevance_boost() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![
        make_result("1", "src/utils.rs", "fn helper() {}", 1.0, None),
        make_result("2", "src/auth/login.rs", "fn validate() {}", 1.0, None),
    ];

    reranker.rerank("auth", &mut results).await.unwrap();

    // "auth" appears in filepath of result 2
    assert!(
        results[0].chunk.id == "2",
        "Result with path match should be first"
    );
}

#[tokio::test]
async fn test_rerank_recency_boost() {
    let reranker = RuleBasedReranker::new();
    let now = now_timestamp();
    let old_time = now - (30 * 86400); // 30 days ago

    let mut results = vec![
        make_result("1", "old.rs", "fn old() {}", 1.0, Some(old_time)),
        make_result("2", "recent.rs", "fn recent() {}", 1.0, Some(now)),
    ];

    // Query doesn't match any content/path, only recency applies
    reranker.rerank("xyz", &mut results).await.unwrap();

    // Recent file should be boosted
    assert!(
        results[0].chunk.id == "2",
        "Recently modified file should be first"
    );
}

#[tokio::test]
async fn test_rerank_combined_boosts() {
    let reranker = RuleBasedReranker::new();
    let now = now_timestamp();

    let mut results = vec![
        make_result("1", "src/utils.rs", "fn helper() {}", 1.0, None),
        make_result(
            "2",
            "src/auth/login.rs",
            "fn auth_login() {}",
            1.0,
            Some(now),
        ),
    ];

    reranker.rerank("auth login", &mut results).await.unwrap();

    // Result 2 has: exact match (auth + login in content) + path match + recency
    assert!(
        results[0].chunk.id == "2",
        "Result with multiple boosts should be first"
    );

    // Verify boost: exact_match(2.0) * path_relevance(1.5) * recency(1.2) = 3.6
    assert!(
        results[0].score > 3.0,
        "Score should reflect combined boosts: {:.2}",
        results[0].score
    );
}

#[tokio::test]
async fn test_rerank_empty_results() {
    let reranker = RuleBasedReranker::new();
    let mut results: Vec<SearchResult> = vec![];

    // Should not panic
    reranker.rerank("foo", &mut results).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_rerank_empty_query() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![make_result("1", "src/foo.rs", "fn bar() {}", 1.0, None)];
    let original_score = results[0].score;

    reranker.rerank("", &mut results).await.unwrap();

    // Score should not change with empty query
    assert_eq!(results[0].score, original_score);
}

#[tokio::test]
async fn test_rerank_preserves_order_when_no_boosts() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![
        make_result("1", "a.rs", "fn a() {}", 2.0, None),
        make_result("2", "b.rs", "fn b() {}", 1.0, None),
    ];

    // Query doesn't match any content/path
    reranker.rerank("xyz", &mut results).await.unwrap();

    // Original order preserved based on scores
    assert_eq!(results[0].chunk.id, "1");
    assert_eq!(results[1].chunk.id, "2");
}

#[tokio::test]
async fn test_custom_config() {
    let config = RuleBasedRerankerConfig {
        exact_match_boost: 5.0,
        path_relevance_boost: 3.0,
        recency_boost: 2.0,
        recency_days_threshold: 14,
    };
    let reranker = RuleBasedReranker::with_config(config);

    let now = now_timestamp();
    let mut results = vec![make_result(
        "1",
        "src/foo/bar.rs",
        "fn foo_bar() {}",
        1.0,
        Some(now),
    )];

    reranker.rerank("foo bar", &mut results).await.unwrap();

    // exact_match(5.0) * path_relevance(3.0) * recency(2.0) = 30.0
    assert!(results[0].score >= 29.0, "Custom boosts should be applied");
}

#[tokio::test]
async fn test_case_insensitive_matching() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![
        make_result("1", "src/Utils.rs", "fn Helper() {}", 1.0, None),
        make_result("2", "src/other.rs", "fn other() {}", 1.0, None),
    ];

    reranker.rerank("UTILS helper", &mut results).await.unwrap();

    // Case-insensitive match should work
    assert!(
        results[0].chunk.id == "1",
        "Case-insensitive match should boost result"
    );
}

#[tokio::test]
async fn test_partial_term_match() {
    let reranker = RuleBasedReranker::new();
    let mut results = vec![
        make_result("1", "src/authentication.rs", "fn auth() {}", 1.0, None),
        make_result("2", "src/other.rs", "fn other() {}", 1.0, None),
    ];

    reranker.rerank("auth", &mut results).await.unwrap();

    // "auth" is substring of "authentication" in filepath
    assert!(
        results[0].chunk.id == "1",
        "Partial term match in filepath should work"
    );
}

#[test]
fn test_contains_exact_match() {
    let reranker = RuleBasedReranker::new();

    assert!(reranker.contains_exact_match("fn foo_bar() {}", "foo bar"));
    assert!(reranker.contains_exact_match("FN FOO_BAR() {}", "foo bar"));
    assert!(!reranker.contains_exact_match("fn baz() {}", "foo bar"));
    assert!(reranker.contains_exact_match("hello world", "hello"));
}

#[test]
fn test_path_contains_query_terms() {
    let reranker = RuleBasedReranker::new();

    assert!(reranker.path_contains_query_terms("src/auth/login.rs", "auth"));
    assert!(reranker.path_contains_query_terms("src/AUTH/LOGIN.rs", "auth"));
    assert!(!reranker.path_contains_query_terms("src/utils.rs", "auth"));
    assert!(reranker.path_contains_query_terms("tests/integration.rs", "test"));
}
