use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

use super::openai_compatible_completion_api::OpenAICompatibleCompletionUsage;

/// Convert completion usage to SDK `Usage`.
pub fn convert_openai_compatible_completion_usage(
    usage: Option<&OpenAICompatibleCompletionUsage>,
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

    Usage {
        input_tokens: InputTokens::from_uncached(Some(prompt_tokens)),
        output_tokens: OutputTokens {
            total: Some(completion_tokens),
            text: Some(completion_tokens),
            reasoning: None,
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}
