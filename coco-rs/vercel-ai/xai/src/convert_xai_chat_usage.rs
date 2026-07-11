use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Raw usage payload from the xAI Chat Completions API.
///
/// Shared by the non-streaming response body and the streaming top-level
/// `usage` field (emitted when `stream_options.include_usage` is set).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiChatUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub prompt_tokens_details: Option<XaiPromptTokensDetails>,
    pub completion_tokens_details: Option<XaiCompletionTokensDetails>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiPromptTokensDetails {
    pub text_tokens: Option<u64>,
    pub audio_tokens: Option<u64>,
    pub image_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiCompletionTokensDetails {
    pub reasoning_tokens: Option<u64>,
    pub audio_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
}

/// Convert xAI usage to the SDK's unified `Usage` type.
///
/// Mirrors `convert-xai-chat-usage.ts`:
/// - `reasoning_tokens` are treated as *additive* to `completion_tokens`, so
///   `outputTokens.total = completion_tokens + reasoning_tokens` while
///   `outputTokens.text = completion_tokens`.
/// - The prompt total may or may not already include the cache-read bucket;
///   when `cached_tokens > prompt_tokens` the API reports them exclusively, so
///   the total is `prompt_tokens + cached_tokens`.
///
/// Unlike the TS reference (which discards the cache-read bucket for cost) we
/// surface it via `InputTokens`, keeping xAI consistent with every other coco
/// provider and feeding cost tracking. Full raw usage is preserved in `raw`.
pub fn convert_xai_chat_usage(usage: Option<&XaiChatUsage>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };

    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let completion_tokens = usage.completion_tokens.unwrap_or(0);
    let cache_read = usage
        .prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens);
    let reasoning = usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens);

    // When the reported cache-read exceeds the prompt total, the API is
    // reporting them exclusively; otherwise the prompt total is inclusive.
    let prompt_includes_cache = cache_read.unwrap_or(0) <= prompt_tokens;
    let input_tokens = if prompt_includes_cache {
        InputTokens::from_inclusive_total(Some(prompt_tokens), cache_read, None)
    } else {
        InputTokens::from_exclusive_buckets(Some(prompt_tokens), cache_read, None)
    };

    Usage {
        input_tokens,
        output_tokens: OutputTokens {
            total: Some(completion_tokens.saturating_add(reasoning.unwrap_or(0))),
            text: Some(completion_tokens),
            reasoning,
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}

#[cfg(test)]
#[path = "convert_xai_chat_usage.test.rs"]
mod tests;
