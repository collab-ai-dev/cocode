use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

use crate::provider_options::PromptTokensTotalSemantics;

/// Raw usage from an OpenAI-compatible Chat Completions API.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OpenAICompatibleChatUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    pub prompt_cache_hit_tokens: Option<u64>,
    pub prompt_cache_miss_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PromptTokensDetails {
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CompletionTokensDetails {
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
}

/// Convert OpenAI-compatible Chat usage to the SDK's unified `Usage` type.
pub fn convert_openai_compatible_chat_usage(
    usage: Option<&OpenAICompatibleChatUsage>,
    prompt_tokens_total_semantics: PromptTokensTotalSemantics,
) -> Usage {
    let Some(usage) = usage else {
        return Usage {
            input_tokens: InputTokens::default(),
            output_tokens: OutputTokens {
                total: None,
                text: None,
                reasoning: None,
            },
            raw: None,
        };
    };

    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let completion_tokens = usage.completion_tokens.unwrap_or(0);
    let input_tokens = if usage.prompt_cache_hit_tokens.is_some()
        || usage.prompt_cache_miss_tokens.is_some()
    {
        input_tokens_from_deepseek_cache_fields(
            usage.prompt_tokens,
            usage.prompt_cache_hit_tokens,
            usage.prompt_cache_miss_tokens,
        )
    } else {
        let cached_tokens = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        let normalized_prompt_tokens = match prompt_tokens_total_semantics {
            PromptTokensTotalSemantics::Inclusive => prompt_tokens,
            PromptTokensTotalSemantics::NonInclusive => prompt_tokens.saturating_add(cached_tokens),
        };
        InputTokens::from_inclusive_total(Some(normalized_prompt_tokens), Some(cached_tokens), None)
    };
    let reasoning_tokens = usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
        .unwrap_or(0);

    Usage {
        input_tokens,
        output_tokens: OutputTokens {
            total: Some(completion_tokens),
            text: Some(completion_tokens.saturating_sub(reasoning_tokens)),
            reasoning: Some(reasoning_tokens),
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}

fn input_tokens_from_deepseek_cache_fields(
    prompt_tokens: Option<u64>,
    hit_tokens: Option<u64>,
    miss_tokens: Option<u64>,
) -> InputTokens {
    match (hit_tokens, miss_tokens, prompt_tokens) {
        (Some(hit), Some(miss), _) => {
            InputTokens::from_exclusive_buckets(Some(miss), Some(hit), None)
        }
        (Some(hit), None, Some(total)) => {
            InputTokens::from_exclusive_buckets(Some(total.saturating_sub(hit)), Some(hit), None)
        }
        (None, Some(miss), Some(total)) => {
            InputTokens::from_exclusive_buckets(Some(miss), Some(total.saturating_sub(miss)), None)
        }
        (Some(hit), None, None) => input_tokens_from_partial_buckets(None, Some(hit)),
        (None, Some(miss), None) => input_tokens_from_partial_buckets(Some(miss), None),
        (None, None, Some(total)) => InputTokens::from_total(Some(total)),
        (None, None, None) => InputTokens::default(),
    }
}

fn input_tokens_from_partial_buckets(
    no_cache: Option<u64>,
    cache_read: Option<u64>,
) -> InputTokens {
    let mut value = serde_json::Map::new();
    if let Some(no_cache) = no_cache {
        value.insert("noCache".to_string(), serde_json::json!(no_cache));
    }
    if let Some(cache_read) = cache_read {
        value.insert("cacheRead".to_string(), serde_json::json!(cache_read));
    }
    serde_json::from_value(serde_json::Value::Object(value)).unwrap_or_default()
}

#[cfg(test)]
#[path = "convert_chat_usage.test.rs"]
mod tests;
