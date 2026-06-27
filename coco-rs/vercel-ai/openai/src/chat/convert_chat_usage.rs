use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Raw usage from the OpenAI Chat Completions API.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OpenAIChatUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    pub completion_tokens_details: Option<CompletionTokensDetails>,
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

/// Convert OpenAI Chat usage to the SDK's unified `Usage` type.
///
/// OpenAI reports `prompt_tokens` inclusive of cached tokens, so `total`
/// preserves the raw prompt total and `no_cache` subtracts `cached_tokens`.
pub fn convert_openai_chat_usage(usage: Option<&OpenAIChatUsage>) -> Usage {
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
    let cached_tokens = usage
        .prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
        .unwrap_or(0);

    Usage {
        input_tokens: InputTokens::from_inclusive_total(
            Some(prompt_tokens),
            Some(cached_tokens),
            None,
        ),
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

#[cfg(test)]
#[path = "convert_chat_usage.test.rs"]
mod tests;
