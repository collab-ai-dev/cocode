//! Flattened usage types matching TS `LanguageModelUsage`.
//!
//! Provides a user-friendly view of token usage that flattens
//! the provider-level `Usage` type.

use std::collections::HashMap;

use vercel_ai_provider::ImageModelV4Usage;
use vercel_ai_provider::Usage;

use crate::types::JSONValue;

/// Flattened usage type matching TS `LanguageModelUsage`.
///
/// Provides a simpler view of token usage compared to the provider-level
/// `Usage` type with its nested `InputTokens`/`OutputTokens` structs.
#[derive(Debug, Clone, Default)]
pub struct LanguageModelUsage {
    /// Total input tokens consumed.
    pub input_tokens: Option<u64>,
    /// Breakdown of input token usage.
    pub input_token_details: InputTokenDetails,
    /// Total output tokens consumed.
    pub output_tokens: Option<u64>,
    /// Breakdown of output token usage.
    pub output_token_details: OutputTokenDetails,
    /// Total tokens (input + output).
    pub total_tokens: Option<u64>,
    /// Raw usage data from the provider.
    pub raw: Option<HashMap<String, JSONValue>>,
}

/// Breakdown of input token usage.
#[derive(Debug, Clone, Default)]
pub struct InputTokenDetails {
    /// Tokens not from cache.
    pub no_cache_tokens: Option<u64>,
    /// Tokens read from cache.
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to cache.
    pub cache_write_tokens: Option<u64>,
}

/// Breakdown of output token usage.
#[derive(Debug, Clone, Default)]
pub struct OutputTokenDetails {
    /// Tokens used for text output.
    pub text_tokens: Option<u64>,
    /// Tokens used for reasoning.
    pub reasoning_tokens: Option<u64>,
}

/// Convert from provider `Usage` to flattened `LanguageModelUsage`.
pub fn as_language_model_usage(usage: &Usage) -> LanguageModelUsage {
    LanguageModelUsage {
        input_tokens: usage.input_tokens.total(),
        input_token_details: InputTokenDetails {
            no_cache_tokens: usage.input_tokens.no_cache(),
            cache_read_tokens: usage.input_tokens.cache_read(),
            cache_write_tokens: usage.input_tokens.cache_write(),
        },
        output_tokens: usage.output_tokens.total,
        output_token_details: OutputTokenDetails {
            text_tokens: usage.output_tokens.text,
            reasoning_tokens: usage.output_tokens.reasoning,
        },
        total_tokens: add_options(usage.input_tokens.total(), usage.output_tokens.total),
        raw: usage.raw.clone(),
    }
}

/// Sum two `LanguageModelUsage` values.
pub fn add_language_model_usage(
    a: &LanguageModelUsage,
    b: &LanguageModelUsage,
) -> LanguageModelUsage {
    LanguageModelUsage {
        input_tokens: add_options(a.input_tokens, b.input_tokens),
        input_token_details: InputTokenDetails {
            no_cache_tokens: add_options(
                a.input_token_details.no_cache_tokens,
                b.input_token_details.no_cache_tokens,
            ),
            cache_read_tokens: add_options(
                a.input_token_details.cache_read_tokens,
                b.input_token_details.cache_read_tokens,
            ),
            cache_write_tokens: add_options(
                a.input_token_details.cache_write_tokens,
                b.input_token_details.cache_write_tokens,
            ),
        },
        output_tokens: add_options(a.output_tokens, b.output_tokens),
        output_token_details: OutputTokenDetails {
            text_tokens: add_options(
                a.output_token_details.text_tokens,
                b.output_token_details.text_tokens,
            ),
            reasoning_tokens: add_options(
                a.output_token_details.reasoning_tokens,
                b.output_token_details.reasoning_tokens,
            ),
        },
        total_tokens: add_options(a.total_tokens, b.total_tokens),
        raw: None,
    }
}

/// Create a null/empty usage value.
pub fn create_null_language_model_usage() -> LanguageModelUsage {
    LanguageModelUsage::default()
}

/// Sum two `ImageModelUsage` values.
pub fn add_image_model_usage(a: &ImageModelV4Usage, b: &ImageModelV4Usage) -> ImageModelV4Usage {
    ImageModelV4Usage {
        prompt_tokens: a.prompt_tokens.saturating_add(b.prompt_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        total_tokens: a.total_tokens.saturating_add(b.total_tokens),
    }
}

fn add_options(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "usage.test.rs"]
mod tests;
