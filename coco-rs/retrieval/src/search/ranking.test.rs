use super::*;
use crate::types::CodeChunk;
use crate::types::ScoreType;

#[test]
fn test_extract_symbols() {
    let symbols = extract_symbols("fn get_user_name(id: i32) -> String");
    assert!(symbols.contains("fn"));
    assert!(symbols.contains("get"));
    assert!(symbols.contains("user"));
    assert!(symbols.contains("name"));
    assert!(symbols.contains("id"));
    assert!(symbols.contains("i32"));
    assert!(symbols.contains("string"));
}

#[test]
fn test_extract_symbols_code() {
    let code = "let result = calculate_sum(a, b);";
    let symbols = extract_symbols(code);
    assert!(symbols.contains("let"));
    assert!(symbols.contains("result"));
    assert!(symbols.contains("calculate"));
    assert!(symbols.contains("sum"));
    assert!(symbols.contains("a"));
    assert!(symbols.contains("b"));
}

#[test]
fn test_jaccard_identical() {
    let similarity = jaccard_similarity("hello world", "hello world");
    assert!((similarity - 1.0).abs() < 0.001);
}

#[test]
fn test_jaccard_no_overlap() {
    let similarity = jaccard_similarity("hello world", "foo bar");
    assert!(similarity < 0.001);
}

#[test]
fn test_jaccard_partial_overlap() {
    // "hello world" -> {hello, world}
    // "hello foo" -> {hello, foo}
    // intersection = {hello}, union = {hello, world, foo}
    // similarity = 1/3 = 0.333...
    let similarity = jaccard_similarity("hello world", "hello foo");
    assert!((similarity - 1.0 / 3.0).abs() < 0.01);
}

#[test]
fn test_jaccard_empty() {
    let similarity = jaccard_similarity("", "");
    assert!(similarity < 0.001);

    let similarity = jaccard_similarity("hello", "");
    assert!(similarity < 0.001);
}

#[test]
fn test_jaccard_case_insensitive() {
    let similarity = jaccard_similarity("Hello World", "hello world");
    assert!((similarity - 1.0).abs() < 0.001);
}

fn make_result(content: &str, score: f32) -> SearchResult {
    SearchResult {
        chunk: CodeChunk {
            id: "test".to_string(),
            source_id: "test".to_string(),
            filepath: "test.rs".to_string(),
            language: "rust".to_string(),
            content: content.to_string(),
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
        score_type: ScoreType::Bm25,
        is_stale: None,
    }
}

#[test]
fn test_apply_jaccard_boost() {
    let mut results = vec![
        make_result("fn get_user_name()", 0.5),
        make_result("fn calculate_sum()", 0.5),
    ];

    apply_jaccard_boost(&mut results, "get user", 0.1);

    // First result should have higher score (more overlap with query)
    assert!(results[0].score > results[1].score);
}

#[test]
fn test_rerank_by_jaccard() {
    let mut results = vec![
        make_result("fn calculate_sum(a, b)", 0.5),
        make_result("fn get_user_name(id)", 0.5), // same score
    ];

    rerank_by_jaccard(&mut results, "get user name");

    // Second result (get_user_name) should be first after reranking
    assert!(results[0].chunk.content.contains("get_user_name"));
}
