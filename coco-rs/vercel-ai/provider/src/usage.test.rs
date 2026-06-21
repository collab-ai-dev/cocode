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
fn test_usage_with_details() {
    let input = InputTokens::from_exclusive_buckets(
        /*no_cache*/ Some(50),
        /*cache_read*/ Some(30),
        /*cache_write*/ Some(20),
    );
    let output = OutputTokens {
        total: Some(50),
        text: Some(30),
        reasoning: Some(20),
    };

    let usage = Usage::empty()
        .with_input_tokens(input)
        .with_output_tokens(output);

    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(50));
    assert_eq!(usage.output_tokens.total, Some(50));
    assert_eq!(usage.output_tokens.reasoning, Some(20));
}

#[test]
fn test_usage_serialization() {
    let usage = Usage::new(100, 50);
    let json = serde_json::to_string(&usage).unwrap();
    let parsed: Usage = serde_json::from_str(&json).unwrap();

    assert_eq!(usage, parsed);
}

#[test]
fn test_usage_default() {
    let usage = Usage::default();
    assert_eq!(usage.total_input_tokens(), 0);
    assert_eq!(usage.total_output_tokens(), 0);
}
