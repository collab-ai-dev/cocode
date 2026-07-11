use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Raw usage payload from the xAI Responses API.
///
/// Shared by the non-streaming response body and the streaming
/// `response.completed` / `response.done` / `response.incomplete` events.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiResponsesUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub input_tokens_details: Option<XaiResponsesInputTokensDetails>,
    pub output_tokens_details: Option<XaiResponsesOutputTokensDetails>,
    pub num_sources_used: Option<u64>,
    pub num_server_side_tools_used: Option<u64>,
    pub cost_in_usd_ticks: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiResponsesInputTokensDetails {
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct XaiResponsesOutputTokensDetails {
    pub reasoning_tokens: Option<u64>,
}

/// Convert xAI Responses usage to the SDK's unified `Usage` type.
///
/// Mirrors `convert-xai-responses-usage.ts`:
/// - `reasoning_tokens` are *inclusive* in `output_tokens`, so
///   `outputTokens.text = output_tokens - reasoning_tokens`.
/// - The input total may or may not already include the cache-read bucket;
///   when `cached_tokens > input_tokens` the API reports them exclusively, so
///   the total is `input_tokens + cached_tokens` (mirrors the chat converter).
///
/// Unlike the TS reference (which discards the cache-read bucket for cost) we
/// surface it via `InputTokens`, keeping xAI consistent with every other coco
/// provider. Full raw usage is preserved in `raw`.
pub fn convert_xai_responses_usage(usage: &XaiResponsesUsage) -> Usage {
    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let cache_read = usage
        .input_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens);
    let reasoning = usage
        .output_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens);

    // When the reported cache-read exceeds the input total, the API is
    // reporting them exclusively; otherwise the input total is inclusive.
    let input_includes_cache = cache_read.unwrap_or(0) <= input_tokens;
    let input = if input_includes_cache {
        InputTokens::from_inclusive_total(Some(input_tokens), cache_read, None)
    } else {
        InputTokens::from_exclusive_buckets(Some(input_tokens), cache_read, None)
    };

    Usage {
        input_tokens: input,
        output_tokens: OutputTokens {
            total: Some(output_tokens),
            text: Some(output_tokens.saturating_sub(reasoning.unwrap_or(0))),
            reasoning,
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            v.as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }),
    }
}

#[cfg(test)]
#[path = "convert_xai_responses_usage.test.rs"]
mod tests;
