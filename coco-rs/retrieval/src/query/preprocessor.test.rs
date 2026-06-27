use super::*;

#[test]
fn test_normalize_whitespace() {
    assert_eq!(normalize_whitespace("hello   world"), "hello world");
    assert_eq!(normalize_whitespace("  leading"), "leading");
    assert_eq!(normalize_whitespace("trailing  "), "trailing");
}

#[test]
fn test_tokenize() {
    let tokens = tokenize("hello, world! How are you?");
    assert_eq!(tokens, vec!["hello", "world", "How", "are", "you"]);
}

#[test]
fn test_contains_chinese() {
    assert!(contains_chinese("用户认证"));
    assert!(contains_chinese("hello 世界"));
    assert!(!contains_chinese("hello world"));
}

#[test]
fn test_process_natural_language_query() {
    let config = SearchConfig::default();
    let preprocessor = QueryPreprocessor::new(config);
    let result = preprocessor.process("how to authenticate users");

    // Should be detected as natural language
    assert_eq!(result.query_type, QueryType::NaturalLanguage);
    // "how" and "to" should be removed as stop words
    assert!(!result.tokens.contains(&"how".to_string()));
    assert!(!result.tokens.contains(&"to".to_string()));
    // "authenticate" and "users" should remain (possibly stemmed)
    assert!(result.tokens.iter().any(|t| t.contains("authent")));
    assert!(result.tokens.iter().any(|t| t.contains("user")));
}

#[test]
fn test_is_code_identifier() {
    // Snake case
    assert!(is_code_identifier("get_user_by_id"));
    assert!(is_code_identifier("MAX_SIZE"));

    // CamelCase / PascalCase
    assert!(is_code_identifier("getUserById"));
    assert!(is_code_identifier("GetUserById"));
    assert!(is_code_identifier("XMLParser"));

    // Short identifiers (2 chars or less)
    assert!(is_code_identifier("id"));
    assert!(is_code_identifier("db"));
    assert!(is_code_identifier("io"));

    // Pure lowercase words (3+ chars) are natural language, not identifiers
    assert!(!is_code_identifier("main")); // Treated as natural language now
    assert!(!is_code_identifier("foo"));
    assert!(!is_code_identifier("error"));
    assert!(!is_code_identifier("help"));
    assert!(!is_code_identifier("find"));

    // Not identifiers
    assert!(!is_code_identifier("get user name"));
    assert!(!is_code_identifier("how to parse json"));
    assert!(!is_code_identifier(""));
    assert!(!is_code_identifier("123abc"));
}

#[test]
fn test_has_symbol_syntax() {
    assert!(has_symbol_syntax("type:function"));
    assert!(has_symbol_syntax("name:parse"));
    assert!(has_symbol_syntax("file:src/main.rs"));
    assert!(has_symbol_syntax("path:*.rs"));
    assert!(has_symbol_syntax("type:function name:getUserById"));

    assert!(!has_symbol_syntax("parse error"));
    assert!(!has_symbol_syntax("getUserById"));
}

#[test]
fn test_process_code_identifier() {
    let config = SearchConfig::default();
    let preprocessor = QueryPreprocessor::new(config);

    // Test camelCase
    let result = preprocessor.process("getUserById");
    assert_eq!(result.query_type, QueryType::CodeIdentifier);
    // Should NOT be stemmed
    assert!(result.tokens.contains(&"getUserById".to_string()));
    // Should have trigrams
    assert!(!result.trigrams.is_empty());

    // Test snake_case
    let result = preprocessor.process("get_user_by_id");
    assert_eq!(result.query_type, QueryType::CodeIdentifier);
    assert!(result.tokens.contains(&"get_user_by_id".to_string()));
    // "by" should NOT be removed as stopword in code queries
    assert!(result.tokens.iter().any(|t| t == "by"));
}

#[test]
fn test_process_symbol_search() {
    let config = SearchConfig::default();
    let preprocessor = QueryPreprocessor::new(config);

    let result = preprocessor.process("type:function name:parse");
    assert_eq!(result.query_type, QueryType::SymbolSearch);
    assert!(result.tokens.contains(&"function".to_string()));
    assert!(result.tokens.contains(&"parse".to_string()));
}

#[test]
fn test_tokenize_code_identifier() {
    // CamelCase
    let tokens = tokenize_code_identifier("getUserById");
    assert!(tokens.contains(&"getUserById".to_string()));
    assert!(tokens.contains(&"get".to_string()));
    assert!(tokens.contains(&"User".to_string()));

    // Snake case
    let tokens = tokenize_code_identifier("get_user_by_id");
    assert!(tokens.contains(&"get_user_by_id".to_string()));
    assert!(tokens.contains(&"get".to_string()));
    assert!(tokens.contains(&"user".to_string()));
    assert!(tokens.contains(&"by".to_string()));
    assert!(tokens.contains(&"id".to_string()));
}

#[test]
fn test_generate_trigrams() {
    let trigrams = generate_trigrams("getUserById");
    assert!(trigrams.contains(&"get".to_string()));
    assert!(trigrams.contains(&"etu".to_string())); // lowercase
    assert!(trigrams.contains(&"tus".to_string()));
}

#[test]
fn test_extract_symbol_search_terms() {
    let terms = extract_symbol_search_terms("type:function name:parse file:src/main.rs");
    assert_eq!(terms, vec!["function", "parse", "src/main.rs"]);

    let terms = extract_symbol_search_terms("find name:getUserById");
    assert_eq!(terms, vec!["find", "getUserById"]);
}

#[test]
fn test_query_type_detection() {
    // Code identifiers with clear patterns
    assert_eq!(
        QueryPreprocessor::detect_query_type("getUserById"),
        QueryType::CodeIdentifier
    );
    assert_eq!(
        QueryPreprocessor::detect_query_type("get_user_by_id"),
        QueryType::CodeIdentifier
    );
    assert_eq!(
        QueryPreprocessor::detect_query_type("MAX_SIZE"),
        QueryType::CodeIdentifier
    );

    // Symbol search
    assert_eq!(
        QueryPreprocessor::detect_query_type("type:function"),
        QueryType::SymbolSearch
    );

    // Natural language
    assert_eq!(
        QueryPreprocessor::detect_query_type("how to parse json"),
        QueryType::NaturalLanguage
    );
    // Single lowercase words are now natural language
    assert_eq!(
        QueryPreprocessor::detect_query_type("error"),
        QueryType::NaturalLanguage
    );
    assert_eq!(
        QueryPreprocessor::detect_query_type("main"),
        QueryType::NaturalLanguage
    );

    // Short identifiers stay as code identifiers
    assert_eq!(
        QueryPreprocessor::detect_query_type("id"),
        QueryType::CodeIdentifier
    );
}
