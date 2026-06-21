use super::*;

#[test]
fn test_rewritten_query_unchanged() {
    let query = RewrittenQuery::unchanged("test query");
    assert_eq!(query.original, "test query");
    assert_eq!(query.rewritten, "test query");
    assert!(!query.was_translated);
}

#[test]
fn test_rewritten_query_translated() {
    let query = RewrittenQuery::translated("用户认证", "user authentication");
    assert_eq!(query.original, "用户认证");
    assert_eq!(query.rewritten, "user authentication");
    assert!(query.was_translated);
}

#[test]
fn test_effective_query_with_expansions() {
    let query = RewrittenQuery::unchanged("test function")
        .with_expansions(vec!["fn".to_string(), "method".to_string()]);
    assert_eq!(query.effective_query(), "test function fn method");
}

#[tokio::test]
async fn test_simple_rewriter() {
    let rewriter = SimpleRewriter::new();

    // English query - no translation
    let result = rewriter.rewrite("find user authentication").await.unwrap();
    assert!(!result.was_translated);

    // Query with expansion
    let rewriter = SimpleRewriter::new().with_expansion(true);
    let result = rewriter.rewrite("test function").await.unwrap();
    assert!(!result.expansions.is_empty());
}

#[test]
fn test_query_expansion() {
    let rewriter = SimpleRewriter::new();
    let expansions = rewriter.expand_query("find authentication function");

    assert!(expansions.contains(&"fn".to_string()));
    assert!(expansions.contains(&"login".to_string()));
}

#[test]
fn test_case_variants() {
    let variants = generate_case_variants("get user info");

    // Should have camelCase, PascalCase, snake_case, kebab-case
    let texts: Vec<&str> = variants.iter().map(|(t, _)| t.as_str()).collect();
    assert!(texts.contains(&"getUserInfo"));
    assert!(texts.contains(&"GetUserInfo"));
    assert!(texts.contains(&"get_user_info"));
    assert!(texts.contains(&"get-user-info"));
}

#[test]
fn test_case_variants_single_word() {
    // Single word shouldn't generate variants
    let variants = generate_case_variants("function");
    assert!(variants.is_empty());
}

#[test]
fn test_abbreviations() {
    let abbrevs = generate_abbreviations("user authentication configuration");
    assert!(abbrevs.contains(&"auth".to_string()));
    assert!(abbrevs.contains(&"config".to_string()));
}

#[test]
fn test_abbreviation_expansion() {
    // When query has abbreviation, should expand to full word
    let abbrevs = generate_abbreviations("db connection");
    assert!(abbrevs.contains(&"database".to_string()));
}

#[test]
fn test_structured_expansion() {
    let rewriter = SimpleRewriter::new()
        .with_expansion(true)
        .with_case_variants(true);

    let expansions = rewriter.expand_query_structured("get user info");

    // Should have case variants
    assert!(expansions.iter().any(|e| e.text == "getUserInfo"));
    assert!(
        expansions
            .iter()
            .any(|e| e.expansion_type == ExpansionType::CamelCase)
    );
    assert!(
        expansions
            .iter()
            .any(|e| e.expansion_type == ExpansionType::SnakeCase)
    );

    // Should have synonym expansions for "user"
    assert!(expansions.iter().any(|e| e.text == "account"));
}

#[test]
fn test_capitalize_first() {
    assert_eq!(capitalize_first("hello"), "Hello");
    assert_eq!(capitalize_first("HELLO"), "Hello");
    assert_eq!(capitalize_first(""), "");
    assert_eq!(capitalize_first("a"), "A");
}

#[test]
fn test_custom_synonyms() {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "handler".to_string(),
        vec!["processor".to_string(), "worker".to_string()],
    );

    let rewriter = SimpleRewriter::new().with_custom_synonyms(custom);
    let expansions = rewriter.expand_query("event handler");

    assert!(expansions.contains(&"processor".to_string()));
    assert!(expansions.contains(&"worker".to_string()));
}
