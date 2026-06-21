use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Raw usage from the OpenAI Responses API.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OpenAIResponsesUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub input_tokens_details: Option<InputTokensDetails>,
    pub output_tokens_details: Option<OutputTokensDetails>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct InputTokensDetails {
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OutputTokensDetails {
    pub reasoning_tokens: Option<u64>,
}

/// Convert OpenAI Responses usage to the SDK's unified `Usage` type.
///
/// OpenAI reports `input_tokens` inclusive of cached tokens, so `total`
/// preserves the raw input total and `no_cache` subtracts `cached_tokens`.
pub fn convert_openai_responses_usage(usage: Option<&OpenAIResponsesUsage>) -> Usage {
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

    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let cached = usage
        .input_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    let reasoning = usage
        .output_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
        .unwrap_or(0);

    Usage {
        input_tokens: InputTokens::from_inclusive_total(Some(input_tokens), Some(cached), None),
        output_tokens: OutputTokens {
            total: Some(output_tokens),
            text: Some(output_tokens.saturating_sub(reasoning)),
            reasoning: Some(reasoning),
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}

#[cfg(test)]
#[path = "convert_responses_usage.test.rs"]
mod tests;
