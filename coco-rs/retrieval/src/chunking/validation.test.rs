use super::*;

#[test]
fn test_count_tokens() {
    let counter = TokenCounter::new();

    // Simple text
    let tokens = counter.count_tokens("Hello, world!");
    assert!(tokens > 0);
    assert!(tokens < 10);

    // Longer text should have more tokens
    let long_text = "The quick brown fox jumps over the lazy dog. ".repeat(10);
    let long_tokens = counter.count_tokens(&long_text);
    assert!(long_tokens > tokens);
}

#[test]
fn test_is_valid() {
    let counter = TokenCounter::with_max_tokens(10);

    assert!(counter.is_valid("Hello"));
    assert!(!counter.is_valid(&"word ".repeat(100)));
}

#[test]
fn test_max_tokens() {
    let counter = TokenCounter::with_max_tokens(256);
    assert_eq!(counter.max_tokens(), 256);
}
