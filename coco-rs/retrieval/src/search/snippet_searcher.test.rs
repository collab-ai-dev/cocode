use super::*;
use tempfile::TempDir;

fn make_test_snippet(id: i64, name: &str, syntax_type: &str) -> StoredSnippet {
    StoredSnippet {
        id,
        workspace: "test".to_string(),
        filepath: "src/main.rs".to_string(),
        name: name.to_string(),
        syntax_type: syntax_type.to_string(),
        start_line: (id * 10) as i32,
        end_line: (id * 10 + 5) as i32,
        signature: Some(format!("fn {}()", name)),
        docs: None,
        content_hash: "abc123".to_string(),
    }
}

#[test]
fn test_snippet_to_result() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let searcher = SnippetSearcher::new(store, "test");

    let snippet = make_test_snippet(1, "parse_config", "function");
    let result = searcher.snippet_to_result(snippet, 0);

    assert_eq!(result.chunk.filepath, "src/main.rs");
    assert_eq!(result.chunk.content, "fn parse_config()");
    assert_eq!(result.chunk.start_line, 10);
    assert_eq!(result.chunk.language, "rust");
    assert_eq!(result.score, 1.0);
    assert_eq!(result.score_type, ScoreType::Snippet);
}

#[test]
fn test_rank_based_scores() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let searcher = SnippetSearcher::new(store, "test");

    let snippets = vec![
        make_test_snippet(1, "first", "function"),
        make_test_snippet(2, "second", "function"),
        make_test_snippet(3, "third", "function"),
    ];

    let results = searcher.snippets_to_results(snippets);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].score, 1.0); // rank 0 -> 1.0
    assert_eq!(results[1].score, 0.5); // rank 1 -> 0.5
    assert!((results[2].score - 0.333).abs() < 0.01); // rank 2 -> ~0.33
}

#[test]
fn test_should_use_snippet_search() {
    assert!(SnippetSearcher::should_use_snippet_search("type:function"));
    assert!(SnippetSearcher::should_use_snippet_search("name:parse"));
    assert!(SnippetSearcher::should_use_snippet_search(
        "type:class name:User"
    ));
    assert!(!SnippetSearcher::should_use_snippet_search("parse error"));
    assert!(!SnippetSearcher::should_use_snippet_search("getUserName"));
}

#[test]
fn test_detect_language() {
    assert_eq!(detect_language_from_path("src/main.rs"), "rust");
    assert_eq!(detect_language_from_path("pkg/server.go"), "go");
    assert_eq!(detect_language_from_path("app.py"), "python");
    assert_eq!(detect_language_from_path("App.tsx"), "typescript");
    assert_eq!(detect_language_from_path("unknown.xyz"), "xyz");
    assert_eq!(detect_language_from_path("no_extension"), "text");
}
