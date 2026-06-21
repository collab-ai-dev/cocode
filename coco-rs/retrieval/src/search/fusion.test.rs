use super::*;

fn make_result(id: &str, score: f32, score_type: ScoreType) -> SearchResult {
    SearchResult {
        chunk: CodeChunk {
            id: id.to_string(),
            source_id: "test".to_string(),
            filepath: "test.rs".to_string(),
            language: "rust".to_string(),
            content: "test content".to_string(),
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
        score,
        score_type,
        is_stale: None,
    }
}

fn make_result_with_mtime(id: &str, score: f32, mtime: Option<i64>) -> SearchResult {
    SearchResult {
        chunk: CodeChunk {
            id: id.to_string(),
            source_id: "test".to_string(),
            filepath: "test.rs".to_string(),
            language: "rust".to_string(),
            content: "test content".to_string(),
            start_line: 1,
            end_line: 1,
            embedding: None,
            modified_time: mtime,
            workspace: "test".to_string(),
            content_hash: String::new(),
            indexed_at: 0,
            parent_symbol: None,
            is_overview: false,
        },
        score,
        score_type: ScoreType::Bm25,
        is_stale: None,
    }
}

#[test]
fn test_rrf_score() {
    // At rank 0 with k=60, score = weight / 60
    assert!((rrf_score(0, 1.0, 60.0) - 1.0 / 60.0).abs() < 0.001);
    // At rank 1 with k=60, score = weight / 61
    assert!((rrf_score(1, 1.0, 60.0) - 1.0 / 61.0).abs() < 0.001);
}

#[test]
fn test_fuse_results() {
    let bm25 = vec![
        make_result("a", 1.0, ScoreType::Bm25),
        make_result("b", 0.8, ScoreType::Bm25),
    ];
    let vector = vec![
        make_result("b", 0.9, ScoreType::Vector),
        make_result("c", 0.7, ScoreType::Vector),
    ];

    let config = RrfConfig::default();
    let fused = fuse_results(&bm25, &vector, &[], &config, 10);

    // "b" should be ranked higher because it appears in both lists
    assert_eq!(fused.len(), 3);
    assert_eq!(fused[0].chunk.id, "b");
}

#[test]
fn test_is_identifier_query() {
    // Snake case
    assert!(is_identifier_query("get_user_name"));
    assert!(is_identifier_query("MAX_SIZE"));

    // CamelCase / PascalCase
    assert!(is_identifier_query("getUserName"));
    assert!(is_identifier_query("GetUserName"));
    assert!(is_identifier_query("XMLParser"));

    // Simple identifiers
    assert!(is_identifier_query("main"));
    assert!(is_identifier_query("foo"));

    // Not identifiers
    assert!(!is_identifier_query("get user name"));
    assert!(!is_identifier_query("how to parse json"));
    assert!(!is_identifier_query(""));
    assert!(!is_identifier_query("123abc"));
}

#[test]
fn test_config_for_identifier() {
    let config = RrfConfig::default().for_identifier_query();
    assert_eq!(config.snippet_weight, 0.3);
    assert!(config.snippet_weight > RrfConfig::default().snippet_weight);
}

#[test]
fn test_has_symbol_syntax() {
    // type: prefix
    assert!(has_symbol_syntax("type:function"));
    assert!(has_symbol_syntax("type:class name:User"));

    // name: prefix
    assert!(has_symbol_syntax("name:parse"));
    assert!(has_symbol_syntax("find name:getUserName"));

    // file: prefix
    assert!(has_symbol_syntax("file:src/main.rs"));
    assert!(has_symbol_syntax("type:function file:lib.rs"));

    // path: prefix (alias for file:)
    assert!(has_symbol_syntax("path:src/main.rs"));
    assert!(has_symbol_syntax("path:*.rs type:struct"));

    // Not symbol syntax
    assert!(!has_symbol_syntax("parse error"));
    assert!(!has_symbol_syntax("getUserName"));
    assert!(!has_symbol_syntax("how to fix bug"));
}

#[test]
fn test_config_for_symbol_query() {
    let config = RrfConfig::default().for_symbol_query();
    assert_eq!(config.snippet_weight, 0.6);
    assert_eq!(config.bm25_weight, 0.2);
    assert_eq!(config.vector_weight, 0.1);
}

#[test]
fn test_recency_score_none() {
    assert!(recency_score(None, 7.0) < 0.001);
}

#[test]
fn test_recency_score_now() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let score = recency_score(Some(now), 7.0);
    assert!((score - 1.0).abs() < 0.01); // Should be very close to 1.0
}

#[test]
fn test_recency_score_half_life() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let seven_days_ago = now - (7 * 86400);
    let score = recency_score(Some(seven_days_ago), 7.0);
    assert!((score - 0.5).abs() < 0.01); // Should be ~0.5 after one half-life
}

#[test]
fn test_recency_score_future() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let future = now + 86400;
    assert!(recency_score(Some(future), 7.0) < 0.001);
}

#[test]
fn test_apply_recency_boost() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut results = vec![
        make_result_with_mtime("old", 0.5, Some(now - 30 * 86400)), // 30 days old
        make_result_with_mtime("new", 0.5, Some(now)),              // just now
    ];

    let config = RrfConfig::default().with_recency_boost(0.1);
    apply_recency_boost(&mut results, &config);

    // New file should have higher score
    assert!(results[1].score > results[0].score);
}

#[test]
fn test_recency_boost_disabled() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut results = vec![make_result_with_mtime("test", 0.5, Some(now))];

    let config = RrfConfig::default(); // recency_boost_weight = 0.0
    let original_score = results[0].score;
    apply_recency_boost(&mut results, &config);

    assert!((results[0].score - original_score).abs() < 0.001);
}

#[test]
fn test_config_with_recency_boost() {
    let config = RrfConfig::default().with_recency_boost(0.15);
    assert!((config.recency_boost_weight - 0.15).abs() < 0.001);
    assert!((config.recency_half_life_days - 7.0).abs() < 0.001);

    let config = RrfConfig::default().with_recency_boost_config(0.2, 14.0);
    assert!((config.recency_boost_weight - 0.2).abs() < 0.001);
    assert!((config.recency_half_life_days - 14.0).abs() < 0.001);
}

#[test]
fn test_fuse_all_results() {
    let bm25 = vec![
        make_result("a", 1.0, ScoreType::Bm25),
        make_result("b", 0.8, ScoreType::Bm25),
    ];
    let vector = vec![
        make_result("b", 0.9, ScoreType::Vector),
        make_result("c", 0.7, ScoreType::Vector),
    ];
    let recent = vec![
        make_result("d", 0.95, ScoreType::Hybrid),
        make_result("a", 0.85, ScoreType::Hybrid),
    ];

    let config = RrfConfig::default().with_recent_weight(0.2);
    let fused = fuse_all_results(&bm25, &vector, &[], &recent, &config, 10);

    // All unique items should be present
    assert_eq!(fused.len(), 4);
    // "b" should be ranked high (appears in bm25 and vector)
    // "a" should also be high (appears in bm25 and recent)
    let ids: Vec<_> = fused.iter().map(|r| r.chunk.id.as_str()).collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"b"));
    assert!(ids.contains(&"c"));
    assert!(ids.contains(&"d"));
}

// ========== Additional edge case tests ==========

#[test]
fn test_fuse_empty_inputs() {
    let config = RrfConfig::default();

    // All empty
    let fused = fuse_results(&[], &[], &[], &config, 10);
    assert!(fused.is_empty());

    // Only BM25
    let bm25 = vec![make_result("a", 1.0, ScoreType::Bm25)];
    let fused = fuse_results(&bm25, &[], &[], &config, 10);
    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].chunk.id, "a");

    // Only vector
    let vector = vec![make_result("b", 0.9, ScoreType::Vector)];
    let fused = fuse_results(&[], &vector, &[], &config, 10);
    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].chunk.id, "b");
}

#[test]
fn test_fuse_limit_zero() {
    let bm25 = vec![make_result("a", 1.0, ScoreType::Bm25)];
    let config = RrfConfig::default();

    let fused = fuse_results(&bm25, &[], &[], &config, 0);
    assert!(fused.is_empty());
}

#[test]
fn test_fuse_limit_smaller_than_results() {
    let bm25 = vec![
        make_result("a", 1.0, ScoreType::Bm25),
        make_result("b", 0.8, ScoreType::Bm25),
        make_result("c", 0.6, ScoreType::Bm25),
    ];
    let config = RrfConfig::default();

    let fused = fuse_results(&bm25, &[], &[], &config, 2);
    assert_eq!(fused.len(), 2);
}

#[test]
fn test_rrf_score_ordering() {
    // Verify that RRF score decreases with rank
    let config = RrfConfig::default();
    let score_rank0 = rrf_score(0, config.bm25_weight, config.k);
    let score_rank1 = rrf_score(1, config.bm25_weight, config.k);
    let score_rank10 = rrf_score(10, config.bm25_weight, config.k);

    assert!(score_rank0 > score_rank1);
    assert!(score_rank1 > score_rank10);
}

#[test]
fn test_fuse_duplicate_item_accumulates_score() {
    // Item appearing in multiple sources should have accumulated score
    let bm25 = vec![make_result("dup", 1.0, ScoreType::Bm25)];
    let vector = vec![make_result("dup", 0.9, ScoreType::Vector)];
    let snippet = vec![make_result("dup", 0.8, ScoreType::Hybrid)];

    let config = RrfConfig::new(0.5, 0.3, 0.2);
    let fused = fuse_results(&bm25, &vector, &snippet, &config, 10);

    assert_eq!(fused.len(), 1);

    // Score should be sum of RRF contributions from all three sources
    // rank 0 in all three: 0.5/60 + 0.3/60 + 0.2/60 = 1.0/60
    let expected_score = (0.5 + 0.3 + 0.2) / 60.0;
    assert!(
        (fused[0].score - expected_score).abs() < 0.001,
        "Expected score ~{:.4}, got {:.4}",
        expected_score,
        fused[0].score
    );
}

#[test]
fn test_weight_configuration_affects_ranking() {
    // Item A appears in BM25 only, Item B appears in vector only
    let bm25 = vec![make_result("a", 1.0, ScoreType::Bm25)];
    let vector = vec![make_result("b", 0.9, ScoreType::Vector)];

    // High BM25 weight -> A should rank first
    let config_bm25 = RrfConfig::new(0.8, 0.2, 0.0);
    let fused = fuse_results(&bm25, &vector, &[], &config_bm25, 10);
    assert_eq!(fused[0].chunk.id, "a");

    // High vector weight -> B should rank first
    let config_vector = RrfConfig::new(0.2, 0.8, 0.0);
    let fused = fuse_results(&bm25, &vector, &[], &config_vector, 10);
    assert_eq!(fused[0].chunk.id, "b");
}
