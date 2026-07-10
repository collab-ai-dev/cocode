use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Raw usage payload from the Groq Chat Completions API.
///
/// Shared by the non-streaming response body and the streaming
/// `x_groq.usage` field.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GroqUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub prompt_tokens_details: Option<GroqPromptTokensDetails>,
    pub completion_tokens_details: Option<GroqCompletionTokensDetails>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GroqPromptTokensDetails {
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GroqCompletionTokensDetails {
    pub reasoning_tokens: Option<u64>,
}

/// Convert Groq usage to the SDK's unified `Usage` type.
///
/// Completion tokens are split into text vs. reasoning. Unlike the TS
/// reference (which discards `cached_tokens`), we surface the cache-read
/// bucket so it stays consistent with every other coco provider and feeds
/// cost tracking. `prompt_tokens` is inclusive of `cached_tokens`.
pub fn convert_groq_usage(usage: Option<&GroqUsage>) -> Usage {
    let Some(usage) = usage else {
        return Usage {
            input_tokens: InputTokens::default(),
            output_tokens: OutputTokens::default(),
            raw: None,
        };
    };

    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let completion_tokens = usage.completion_tokens.unwrap_or(0);
    let cached_tokens = usage
        .prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens);
    let reasoning_tokens = usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens);
    let text_tokens = match reasoning_tokens {
        Some(r) => completion_tokens.saturating_sub(r),
        None => completion_tokens,
    };

    Usage {
        input_tokens: InputTokens::from_inclusive_total(Some(prompt_tokens), cached_tokens, None),
        output_tokens: OutputTokens {
            total: Some(completion_tokens),
            text: Some(text_tokens),
            reasoning: reasoning_tokens,
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}

#[cfg(test)]
#[path = "convert_groq_usage.test.rs"]
mod tests;
