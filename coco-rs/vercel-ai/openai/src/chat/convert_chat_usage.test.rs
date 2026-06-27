use super::*;

#[test]
fn converts_none_usage() {
    let usage = convert_openai_chat_usage(None);
    assert!(usage.input_tokens.total().is_none());
    assert!(usage.output_tokens.total.is_none());
}

#[test]
fn converts_basic_usage() {
    let raw = OpenAIChatUsage {
        prompt_tokens: Some(100),
        completion_tokens: Some(50),
        total_tokens: Some(150),
        ..Default::default()
    };
    let usage = convert_openai_chat_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(100));
    assert_eq!(usage.input_tokens.cache_read(), Some(0));
    assert_eq!(usage.output_tokens.total, Some(50));
    assert_eq!(usage.output_tokens.text, Some(50));
    assert_eq!(usage.output_tokens.reasoning, Some(0));
}

#[test]
fn converts_usage_with_details() {
    let raw = OpenAIChatUsage {
        prompt_tokens: Some(200),
        completion_tokens: Some(100),
        total_tokens: Some(300),
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: Some(50),
        }),
        completion_tokens_details: Some(CompletionTokensDetails {
            reasoning_tokens: Some(30),
            accepted_prediction_tokens: Some(10),
            rejected_prediction_tokens: Some(5),
        }),
    };
    let usage = convert_openai_chat_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.input_tokens.no_cache(), Some(150));
    assert_eq!(usage.input_tokens.cache_read(), Some(50));
    assert_eq!(usage.output_tokens.total, Some(100));
    assert_eq!(usage.output_tokens.text, Some(70));
    assert_eq!(usage.output_tokens.reasoning, Some(30));
}
