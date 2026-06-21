use super::*;

#[test]
fn test_snake_case_splitting() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("user_service_handler");
    assert_eq!(tokens, vec!["user", "service", "handler"]);
}

#[test]
fn test_camel_case_splitting() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("getUserById");
    assert_eq!(tokens, vec!["get", "user", "by", "id"]);
}

#[test]
fn test_pascal_case_splitting() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("UserServiceHandler");
    assert_eq!(tokens, vec!["user", "service", "handler"]);
}

#[test]
fn test_acronym_splitting() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("HTTPServerConfig");
    assert_eq!(tokens, vec!["http", "server", "config"]);
}

#[test]
fn test_dot_notation() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("self.user.get_name()");
    assert_eq!(tokens, vec!["self", "user", "get", "name"]);
}

#[test]
fn test_path_separator() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("std::collections::HashMap");
    assert_eq!(tokens, vec!["std", "collections", "hash", "map"]);
}

#[test]
fn test_arrow_operator() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("ptr->next->data");
    assert_eq!(tokens, vec!["ptr", "next", "data"]);
}

#[test]
fn test_mixed_code() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("fn get_user_by_id(userId: i32) -> Option<User>");
    // fn is filtered (< 2 chars), i32 becomes [i32]
    assert!(tokens.contains(&"get".to_string()));
    assert!(tokens.contains(&"user".to_string()));
    assert!(tokens.contains(&"by".to_string()));
    assert!(tokens.contains(&"id".to_string()));
    assert!(tokens.contains(&"option".to_string()));
}

#[test]
fn test_real_rust_code() {
    let tokenizer = CodeTokenizer::new();
    let code = r#"
        pub async fn search_bm25(&self, query: &str) -> Result<Vec<SearchResult>> {
            let embedder = self.embedder.read().await;
            let scorer = self.scorer.read().await;
            scorer.matches(&embedder.embed(query))
        }
    "#;
    let tokens = tokenizer.tokenize(code);

    // Should contain key terms
    assert!(tokens.contains(&"pub".to_string()));
    assert!(tokens.contains(&"async".to_string()));
    assert!(tokens.contains(&"search".to_string()));
    assert!(tokens.contains(&"bm25".to_string()));
    assert!(tokens.contains(&"query".to_string()));
    assert!(tokens.contains(&"result".to_string()));
    assert!(tokens.contains(&"embedder".to_string()));
    assert!(tokens.contains(&"scorer".to_string()));
}

#[test]
fn test_filter_short_tokens() {
    let tokenizer = CodeTokenizer::new();
    // Single-char tokens should be filtered
    let tokens = tokenizer.tokenize("a b c foo bar");
    assert!(!tokens.contains(&"a".to_string()));
    assert!(!tokens.contains(&"b".to_string()));
    assert!(!tokens.contains(&"c".to_string()));
    assert!(tokens.contains(&"foo".to_string()));
    assert!(tokens.contains(&"bar".to_string()));
}

#[test]
fn test_lowercase() {
    let tokenizer = CodeTokenizer::new();
    let tokens = tokenizer.tokenize("FOO Bar BAZ");
    assert_eq!(tokens, vec!["foo", "bar", "baz"]);
}

#[test]
fn test_no_lowercase() {
    let tokenizer = CodeTokenizer::with_config(2, false);
    let tokens = tokenizer.tokenize("FOO Bar");
    assert_eq!(tokens, vec!["FOO", "Bar"]);
}
