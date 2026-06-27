use super::*;
use tempfile::TempDir;

struct TestContext {
    _dir: TempDir,
    snippets: SnippetStorage,
}

fn setup() -> TestContext {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let snippets = SnippetStorage::new(store);
    TestContext {
        _dir: dir,
        snippets,
    }
}

#[tokio::test]
async fn test_store_and_retrieve_tags() {
    let ctx = setup();
    let snippets = &ctx.snippets;

    let tags = vec![
        CodeTag {
            name: "main".to_string(),
            kind: TagKind::Function,
            start_line: 0,
            end_line: 5,
            start_byte: 0,
            end_byte: 100,
            signature: Some("fn main()".to_string()),
            docs: Some("Entry point".to_string()),
            is_definition: true,
        },
        CodeTag {
            name: "Point".to_string(),
            kind: TagKind::Struct,
            start_line: 10,
            end_line: 15,
            start_byte: 200,
            end_byte: 300,
            signature: None,
            docs: None,
            is_definition: true,
        },
    ];

    // Store tags
    let count = snippets
        .store_tags("test_ws", "src/main.rs", &tags, "abc123")
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Retrieve by name
    let results = snippets
        .search_by_name("test_ws", "main", 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "main");
    assert_eq!(results[0].syntax_type, "function");

    // Retrieve by type
    let structs = snippets
        .search_by_type("test_ws", TagKind::Struct, 10)
        .await
        .unwrap();
    assert_eq!(structs.len(), 1);
    assert_eq!(structs[0].name, "Point");

    // Count
    let total = snippets.count("test_ws").await.unwrap();
    assert_eq!(total, 2);
}

#[tokio::test]
async fn test_delete_snippets() {
    let ctx = setup();
    let snippets = &ctx.snippets;

    let tags = vec![CodeTag {
        name: "foo".to_string(),
        kind: TagKind::Function,
        start_line: 0,
        end_line: 5,
        start_byte: 0,
        end_byte: 100,
        signature: None,
        docs: None,
        is_definition: true,
    }];

    snippets
        .store_tags("ws", "file.rs", &tags, "hash")
        .await
        .unwrap();

    let deleted = snippets.delete_by_filepath("ws", "file.rs").await.unwrap();
    assert_eq!(deleted, 1);

    let count = snippets.count("ws").await.unwrap();
    assert_eq!(count, 0);
}

// ========== SymbolQuery Unit Tests ==========

#[test]
fn test_parse_type_only() {
    let q = SymbolQuery::parse("type:function");
    assert_eq!(q.kind, Some(TagKind::Function));
    assert_eq!(q.name, None);
    assert_eq!(q.text, None);
    assert!(q.is_symbol_query());
}

#[test]
fn test_parse_name_only() {
    let q = SymbolQuery::parse("name:parse");
    assert_eq!(q.kind, None);
    assert_eq!(q.name, Some("parse".to_string()));
    assert_eq!(q.text, None);
    assert!(q.is_symbol_query());
}

#[test]
fn test_parse_name_with_wildcards() {
    let q = SymbolQuery::parse("name:*parse*");
    assert_eq!(q.name, Some("parse".to_string()));
}

#[test]
fn test_parse_combined() {
    let q = SymbolQuery::parse("type:method name:get");
    assert_eq!(q.kind, Some(TagKind::Method));
    assert_eq!(q.name, Some("get".to_string()));
    assert_eq!(q.text, None);
}

#[test]
fn test_parse_with_text() {
    let q = SymbolQuery::parse("type:function parse error");
    assert_eq!(q.kind, Some(TagKind::Function));
    assert_eq!(q.name, None);
    assert_eq!(q.text, Some("parse error".to_string()));
}

#[test]
fn test_parse_text_only() {
    let q = SymbolQuery::parse("parse error handling");
    assert_eq!(q.kind, None);
    assert_eq!(q.name, None);
    assert_eq!(q.text, Some("parse error handling".to_string()));
    assert!(!q.is_symbol_query());
}

#[test]
fn test_parse_empty() {
    let q = SymbolQuery::parse("");
    assert!(q.is_empty());
    assert!(!q.is_symbol_query());
}

#[test]
fn test_parse_filepath() {
    let q = SymbolQuery::parse("file:src/main.rs type:function");
    assert_eq!(q.filepath, Some("src/main.rs".to_string()));
    assert_eq!(q.kind, Some(TagKind::Function));
    assert_eq!(q.name, None);
}

#[test]
fn test_parse_path_alias() {
    // path: should work the same as file:
    let q = SymbolQuery::parse("path:src/main.rs type:function");
    assert_eq!(q.filepath, Some("src/main.rs".to_string()));
    assert_eq!(q.kind, Some(TagKind::Function));
}

#[test]
fn test_for_file() {
    let q = SymbolQuery::for_file("src/lib.rs");
    assert_eq!(q.filepath, Some("src/lib.rs".to_string()));
    assert!(q.name.is_none());
    assert!(q.kind.is_none());
}

// ========== Query Builder Unit Tests ==========

#[test]
fn test_build_simple_query_parameterized() {
    let pq = build_simple_query(
        "ws",
        &Some("parse".to_string()),
        &Some("function".to_string()),
        &None,
        10,
    );
    // Check SQL uses placeholders
    assert!(pq.sql.contains("workspace = ?1"));
    assert!(pq.sql.contains("name LIKE ?2"));
    assert!(pq.sql.contains("syntax_type = ?3"));
    assert!(pq.sql.contains("LIMIT 10"));
    // Check params
    assert_eq!(pq.params.len(), 3);
    assert_eq!(pq.params[0], "ws");
    assert_eq!(pq.params[1], "%parse%");
    assert_eq!(pq.params[2], "function");
}

#[test]
fn test_build_simple_query_with_filepath() {
    let pq = build_simple_query("ws", &None, &None, &Some("src/main.rs".to_string()), 10);
    assert!(pq.sql.contains("workspace = ?1"));
    assert!(pq.sql.contains("filepath = ?2"));
    assert_eq!(pq.params.len(), 2);
    assert_eq!(pq.params[0], "ws");
    assert_eq!(pq.params[1], "src/main.rs");
}

#[test]
fn test_build_simple_query_with_filepath_pattern() {
    let pq = build_simple_query("ws", &None, &None, &Some("src/*.rs".to_string()), 10);
    assert!(pq.sql.contains("filepath LIKE ?2"));
    assert_eq!(pq.params[1], "src/%.rs");
}

#[test]
fn test_build_fts_query_parameterized() {
    let pq = build_fts_query(
        "ws",
        &None,
        &Some("function".to_string()),
        &None,
        &Some("error handling".to_string()),
        20,
    );
    // Check SQL uses placeholders
    assert!(pq.sql.contains("s.workspace = ?1"));
    assert!(pq.sql.contains("snippets_fts MATCH ?2"));
    assert!(pq.sql.contains("s.syntax_type = ?3"));
    assert!(pq.sql.contains("LIMIT 20"));
    // Check params
    assert_eq!(pq.params.len(), 3);
    assert_eq!(pq.params[0], "ws");
    assert_eq!(pq.params[1], "\"error handling\""); // FTS5 phrase search
    assert_eq!(pq.params[2], "function");
}

#[test]
fn test_fts_escapes_quotes() {
    let pq = build_fts_query(
        "ws",
        &None,
        &None,
        &None,
        &Some("test \"quoted\" value".to_string()),
        10,
    );
    // Quotes should be escaped for FTS5
    assert_eq!(pq.params[1], "\"test \"\"quoted\"\" value\"");
}

#[test]
fn test_build_fts_query_with_filepath() {
    let pq = build_fts_query(
        "ws",
        &None,
        &None,
        &Some("src/lib.rs".to_string()),
        &Some("parse".to_string()),
        10,
    );
    assert!(pq.sql.contains("snippets_fts MATCH ?2"));
    assert!(pq.sql.contains("s.filepath = ?3"));
    assert_eq!(pq.params[2], "src/lib.rs");
}
