use super::*;

#[test]
fn test_usage_new() {
    let usage = Usage::new(100, 50);
    assert_eq!(usage.total_input_tokens(), 100);
    assert_eq!(usage.total_output_tokens(), 50);
    assert_eq!(usage.total_tokens(), 150);
}

#[test]
fn test_usage_empty() {
    let usage = Usage::empty();
    assert_eq!(usage.total_input_tokens(), 0);
    assert_eq!(usage.total_output_tokens(), 0);
    assert_eq!(usage.total_tokens(), 0);
}

#[test]
fn test_usage_default() {
    let usage = Usage::default();
    assert_eq!(usage.total_input_tokens(), 0);
    assert_eq!(usage.total_output_tokens(), 0);
}

#[test]
fn test_usage_with_input_tokens() {
    let input = InputTokens::from_exclusive_buckets(
        /*no_cache*/ Some(60),
        /*cache_read*/ Some(30),
        /*cache_write*/ Some(10),
    );
    let usage = Usage::empty().with_input_tokens(input);
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(60));
    assert_eq!(usage.input_tokens.cache_read(), Some(30));
    assert_eq!(usage.input_tokens.cache_write(), Some(10));
}

#[test]
fn test_input_tokens_total_includes_cache_buckets() {
    let input = InputTokens::from_exclusive_buckets(
        /*no_cache*/ Some(60),
        /*cache_read*/ Some(30),
        /*cache_write*/ Some(10),
    );
    assert_eq!(
        input.total(),
        Some(
            input.no_cache().unwrap_or(0)
                + input.cache_read().unwrap_or(0)
                + input.cache_write().unwrap_or(0)
        )
    );
}

#[test]
fn test_usage_with_output_tokens() {
    let output = OutputTokens {
        total: Some(50),
        text: Some(40),
        reasoning: Some(10),
    };
    let usage = Usage::empty().with_output_tokens(output);
    assert_eq!(usage.output_tokens.total, Some(50));
    assert_eq!(usage.output_tokens.text, Some(40));
    assert_eq!(usage.output_tokens.reasoning, Some(10));
}

#[test]
fn test_input_tokens_default() {
    let tokens = InputTokens::default();
    assert_eq!(tokens.total(), None);
    assert_eq!(tokens.no_cache(), None);
    assert_eq!(tokens.cache_read(), None);
    assert_eq!(tokens.cache_write(), None);
}

#[test]
fn test_output_tokens_default() {
    let tokens = OutputTokens::default();
    assert_eq!(tokens.total, None);
    assert_eq!(tokens.text, None);
    assert_eq!(tokens.reasoning, None);
}

#[test]
fn test_usage_add() {
    let mut usage1 = Usage::new(100, 50);
    let usage2 = Usage::new(200, 80);
    usage1.add(&usage2);
    assert_eq!(usage1.total_input_tokens(), 300);
    assert_eq!(usage1.total_output_tokens(), 130);
}

#[test]
fn test_usage_serialization() {
    let usage = Usage::new(100, 50);
    let json = serde_json::to_string(&usage).unwrap();
    let parsed: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(usage, parsed);
}

#[test]
fn test_usage_camel_case_serialization() {
    let usage = Usage::new(100, 50);
    let json = serde_json::to_value(&usage).unwrap();
    // Should serialize as camelCase
    assert!(json.get("inputTokens").is_some());
    assert!(json.get("outputTokens").is_some());
}
