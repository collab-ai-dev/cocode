use super::*;
use pretty_assertions::assert_eq;

#[test]
fn none_usage_yields_empty() {
    let usage = convert_groq_usage(None);
    assert_eq!(usage.input_tokens.total(), None);
    assert_eq!(usage.output_tokens.total, None);
    assert!(usage.raw.is_none());
}

#[test]
fn splits_reasoning_from_text() {
    let raw = GroqUsage {
        prompt_tokens: Some(100),
        completion_tokens: Some(40),
        total_tokens: Some(140),
        prompt_tokens_details: None,
        completion_tokens_details: Some(GroqCompletionTokensDetails {
            reasoning_tokens: Some(15),
        }),
    };
    let usage = convert_groq_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(100));
    assert_eq!(usage.input_tokens.cache_read(), None);
    assert_eq!(usage.output_tokens.total, Some(40));
    assert_eq!(usage.output_tokens.text, Some(25));
    assert_eq!(usage.output_tokens.reasoning, Some(15));
    assert!(usage.raw.is_some());
}

#[test]
fn surfaces_cached_prompt_tokens() {
    let raw = GroqUsage {
        prompt_tokens: Some(100),
        completion_tokens: Some(10),
        total_tokens: Some(110),
        prompt_tokens_details: Some(GroqPromptTokensDetails {
            cached_tokens: Some(30),
        }),
        completion_tokens_details: None,
    };
    let usage = convert_groq_usage(Some(&raw));
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.cache_read(), Some(30));
    assert_eq!(usage.input_tokens.no_cache(), Some(70));
}

#[test]
fn no_reasoning_details_gives_all_text() {
    let raw = GroqUsage {
        prompt_tokens: Some(10),
        completion_tokens: Some(20),
        total_tokens: Some(30),
        prompt_tokens_details: None,
        completion_tokens_details: None,
    };
    let usage = convert_groq_usage(Some(&raw));
    assert_eq!(usage.output_tokens.total, Some(20));
    assert_eq!(usage.output_tokens.text, Some(20));
    assert_eq!(usage.output_tokens.reasoning, None);
}
