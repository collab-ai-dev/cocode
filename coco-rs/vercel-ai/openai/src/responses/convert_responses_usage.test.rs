use super::*;

#[test]
fn converts_none_usage() {
    let usage = convert_openai_responses_usage(None);
    assert!(usage.input_tokens.total().is_none());
}

#[test]
fn converts_basic_usage() {
    let raw = OpenAIResponsesUsage {
        input_tokens: Some(200),
        output_tokens: Some(80),
        ..Default::default()
    };
    let usage = convert_openai_responses_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.output_tokens.total, Some(80));
    assert_eq!(usage.output_tokens.text, Some(80));
    assert_eq!(usage.output_tokens.reasoning, Some(0));
}

#[test]
fn converts_usage_with_details() {
    let raw = OpenAIResponsesUsage {
        input_tokens: Some(300),
        output_tokens: Some(150),
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: Some(100),
        }),
        output_tokens_details: Some(OutputTokensDetails {
            reasoning_tokens: Some(50),
        }),
    };
    let usage = convert_openai_responses_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(300));
    assert_eq!(usage.input_tokens.no_cache(), Some(200));
    assert_eq!(usage.input_tokens.cache_read(), Some(100));
    assert_eq!(usage.output_tokens.text, Some(100));
    assert_eq!(usage.output_tokens.reasoning, Some(50));
}
