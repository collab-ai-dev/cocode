//! Convert Google Generative AI usage metadata to unified usage.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

/// Per-modality token breakdown (e.g. text, image, audio, video).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleTokenDetail {
    pub modality: String,
    pub token_count: u64,
}

/// Google API usage metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleUsageMetadata {
    #[serde(default)]
    pub prompt_token_count: Option<u64>,
    #[serde(default)]
    pub candidates_token_count: Option<u64>,
    #[serde(default)]
    pub cached_content_token_count: Option<u64>,
    #[serde(default)]
    pub thoughts_token_count: Option<u64>,
    #[serde(default)]
    pub total_token_count: Option<u64>,
    #[serde(default)]
    pub traffic_type: Option<String>,
    /// Per-modality breakdown of prompt tokens.
    #[serde(default)]
    pub prompt_tokens_details: Option<Vec<GoogleTokenDetail>>,
    /// Per-modality breakdown of candidate (output) tokens.
    #[serde(default)]
    pub candidates_tokens_details: Option<Vec<GoogleTokenDetail>>,
}

/// Convert Google usage metadata to unified Usage type.
///
/// Google reports `promptTokenCount` inclusive of cached content, so `total`
/// preserves the raw prompt total and `no_cache` subtracts cached content.
pub fn convert_usage(usage: Option<&GoogleUsageMetadata>) -> Usage {
    let Some(usage) = usage else {
        return Usage::empty();
    };

    let prompt = usage.prompt_token_count.unwrap_or(0);
    let candidates = usage.candidates_token_count.unwrap_or(0);
    let thoughts = usage.thoughts_token_count.unwrap_or(0);

    Usage {
        input_tokens: InputTokens::from_inclusive_total(
            Some(prompt),
            usage.cached_content_token_count,
            None,
        ),
        output_tokens: OutputTokens {
            total: Some(candidates + thoughts),
            text: Some(candidates),
            reasoning: usage.thoughts_token_count,
        },
        raw: serde_json::to_value(usage).ok().and_then(|v| {
            if let serde_json::Value::Object(map) = v {
                Some(
                    map.into_iter()
                        .collect::<HashMap<String, serde_json::Value>>(),
                )
            } else {
                None
            }
        }),
    }
}

#[cfg(test)]
#[path = "convert_google_generative_ai_usage.test.rs"]
mod tests;
