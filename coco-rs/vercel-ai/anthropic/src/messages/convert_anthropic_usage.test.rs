use super::super::anthropic_messages_api::AnthropicUsage;
use super::super::anthropic_messages_api::AnthropicUsageIterationRaw;
use super::*;

#[test]
fn converts_basic_usage() {
    let usage = AnthropicUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        iterations: None,
    };
    let result = convert_anthropic_usage(Some(&usage));
    assert_eq!(result.input_tokens.total(), Some(100));
    assert_eq!(result.input_tokens.no_cache(), Some(100));
    assert_eq!(result.input_tokens.cache_read(), Some(0));
    assert_eq!(result.input_tokens.cache_write(), Some(0));
    assert_eq!(result.output_tokens.total, Some(50));
}

#[test]
fn converts_usage_with_cache() {
    let usage = AnthropicUsage {
        input_tokens: 80,
        output_tokens: 50,
        cache_creation_input_tokens: Some(10),
        cache_read_input_tokens: Some(20),
        iterations: None,
    };
    let result = convert_anthropic_usage(Some(&usage));
    // total = input + cache_creation + cache_read = 80 + 10 + 20 = 110
    assert_eq!(result.input_tokens.total(), Some(110));
    assert_eq!(result.input_tokens.no_cache(), Some(80));
    assert_eq!(result.input_tokens.cache_read(), Some(20));
    assert_eq!(result.input_tokens.cache_write(), Some(10));
}

#[test]
fn converts_usage_with_iterations() {
    let usage = AnthropicUsage {
        input_tokens: 50,
        output_tokens: 20,
        cache_creation_input_tokens: Some(5),
        cache_read_input_tokens: None,
        iterations: Some(vec![
            AnthropicUsageIterationRaw {
                iteration_type: "compaction".into(),
                input_tokens: 100,
                output_tokens: 30,
            },
            AnthropicUsageIterationRaw {
                iteration_type: "message".into(),
                input_tokens: 60,
                output_tokens: 25,
            },
        ]),
    };
    let result = convert_anthropic_usage(Some(&usage));
    // When iterations present, sum across iterations
    assert_eq!(result.input_tokens.no_cache(), Some(160)); // 100 + 60
    assert_eq!(result.output_tokens.total, Some(55)); // 30 + 25
    // total = 160 + 5 + 0 = 165
    assert_eq!(result.input_tokens.total(), Some(165));
}

#[test]
fn returns_empty_for_none() {
    let result = convert_anthropic_usage(None);
    assert!(result.input_tokens.total().is_none());
    assert!(result.output_tokens.total.is_none());
}
