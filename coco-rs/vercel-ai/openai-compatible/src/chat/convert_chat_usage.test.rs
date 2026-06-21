use super::*;
use crate::provider_options::PromptTokensTotalSemantics;

#[test]
fn converts_none_usage() {
    let usage = convert_openai_compatible_chat_usage(None, PromptTokensTotalSemantics::Inclusive);
    assert!(usage.input_tokens.total().is_none());
    assert!(usage.output_tokens.total.is_none());
}

#[test]
fn converts_basic_usage() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(100),
        completion_tokens: Some(50),
        total_tokens: Some(150),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(100));
    assert_eq!(usage.input_tokens.cache_read(), Some(0));
    assert_eq!(usage.output_tokens.total, Some(50));
    assert_eq!(usage.output_tokens.text, Some(50));
    assert_eq!(usage.output_tokens.reasoning, Some(0));
}

#[test]
fn converts_usage_with_details() {
    let raw = OpenAICompatibleChatUsage {
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
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.input_tokens.no_cache(), Some(150));
    assert_eq!(usage.input_tokens.cache_read(), Some(50));
    assert_eq!(usage.output_tokens.total, Some(100));
    assert_eq!(usage.output_tokens.text, Some(70));
    assert_eq!(usage.output_tokens.reasoning, Some(30));
}

#[test]
fn converts_deepseek_top_level_cache_tokens() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(200),
        completion_tokens: Some(10),
        prompt_cache_hit_tokens: Some(80),
        prompt_cache_miss_tokens: Some(120),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.input_tokens.no_cache(), Some(120));
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}

#[test]
fn converts_deepseek_cache_hit_with_prompt_total() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(200),
        completion_tokens: Some(10),
        prompt_cache_hit_tokens: Some(80),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.input_tokens.no_cache(), Some(120));
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}

#[test]
fn converts_deepseek_cache_miss_with_prompt_total() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(200),
        completion_tokens: Some(10),
        prompt_cache_miss_tokens: Some(120),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(200));
    assert_eq!(usage.input_tokens.no_cache(), Some(120));
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}

#[test]
fn converts_deepseek_cache_hit_greater_than_prompt_total_saturating() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(50),
        completion_tokens: Some(10),
        prompt_cache_hit_tokens: Some(80),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), Some(80));
    assert_eq!(usage.input_tokens.no_cache(), Some(0));
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}

#[test]
fn converts_deepseek_cache_hit_without_prompt_total_preserves_bucket_only() {
    let raw = OpenAICompatibleChatUsage {
        completion_tokens: Some(10),
        prompt_cache_hit_tokens: Some(80),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), None);
    assert_eq!(usage.input_tokens.no_cache(), None);
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}

#[test]
fn converts_deepseek_cache_miss_without_prompt_total_preserves_bucket_only() {
    let raw = OpenAICompatibleChatUsage {
        completion_tokens: Some(10),
        prompt_cache_miss_tokens: Some(120),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::Inclusive);
    assert_eq!(usage.input_tokens.total(), None);
    assert_eq!(usage.input_tokens.no_cache(), Some(120));
    assert_eq!(usage.input_tokens.cache_read(), None);
}

#[test]
fn normalizes_non_inclusive_cached_tokens() {
    let raw = OpenAICompatibleChatUsage {
        prompt_tokens: Some(20),
        completion_tokens: Some(10),
        total_tokens: Some(30),
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: Some(80),
        }),
        ..Default::default()
    };
    let usage =
        convert_openai_compatible_chat_usage(Some(&raw), PromptTokensTotalSemantics::NonInclusive);
    assert_eq!(usage.input_tokens.total(), Some(100));
    assert_eq!(usage.input_tokens.no_cache(), Some(20));
    assert_eq!(usage.input_tokens.cache_read(), Some(80));
}
