use super::*;
use crate::completion::openai_completion_api::OpenAICompletionUsage;

#[test]
fn convert_none_usage() {
    let usage = convert_openai_completion_usage(None);
    assert!(usage.input_tokens.total().is_none());
    assert!(usage.output_tokens.total.is_none());
    assert!(usage.raw.is_none());
}

#[test]
fn convert_partial_usage_prompt_only() {
    let api_usage = OpenAICompletionUsage {
        prompt_tokens: Some(10),
        completion_tokens: None,
        total_tokens: None,
    };
    let usage = convert_openai_completion_usage(Some(&api_usage));
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.input_tokens.no_cache(), Some(10));
    // When completion_tokens is missing, total is None but text defaults to 0
    assert_eq!(usage.output_tokens.total, None);
    assert_eq!(usage.output_tokens.text, Some(0));
}

#[test]
fn convert_full_usage() {
    let api_usage = OpenAICompletionUsage {
        prompt_tokens: Some(10),
        completion_tokens: Some(20),
        total_tokens: Some(30),
    };
    let usage = convert_openai_completion_usage(Some(&api_usage));
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.input_tokens.no_cache(), Some(10));
    assert!(usage.input_tokens.cache_read().is_none());
    assert!(usage.input_tokens.cache_write().is_none());
    assert_eq!(usage.output_tokens.total, Some(20));
    assert_eq!(usage.output_tokens.text, Some(20));
    assert!(usage.output_tokens.reasoning.is_none());
    assert!(usage.raw.is_some());
}
