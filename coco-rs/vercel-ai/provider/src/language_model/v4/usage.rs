//! Token usage types for model responses.
//!
//! Matches the v4 `LanguageModelV4Usage` type with nested
//! `inputTokens` and `outputTokens` structures.

use serde::Deserialize;
use serde::Serialize;

use crate::json_value::JSONObject;

/// Token usage information for a model response.
///
/// Token usage information for a model response.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Input token breakdown.
    pub input_tokens: InputTokens,
    /// Output token breakdown.
    pub output_tokens: OutputTokens,
    /// Raw usage data from the provider (for pass-through).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<JSONObject>,
}

impl Usage {
    /// Create a new Usage with the given total input/output token counts.
    pub fn new(input_total: u64, output_total: u64) -> Self {
        Self {
            input_tokens: InputTokens::from_total(Some(input_total)),
            output_tokens: OutputTokens {
                total: Some(output_total),
                ..Default::default()
            },
            raw: None,
        }
    }

    /// Create an empty Usage.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Get total input tokens.
    pub fn total_input_tokens(&self) -> u64 {
        self.input_tokens.total().unwrap_or(0)
    }

    /// Get total output tokens.
    pub fn total_output_tokens(&self) -> u64 {
        self.output_tokens.total.unwrap_or(0)
    }

    /// Get total tokens (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens()
            .saturating_add(self.total_output_tokens())
    }

    /// Set input tokens details.
    pub fn with_input_tokens(mut self, input_tokens: InputTokens) -> Self {
        self.input_tokens = input_tokens;
        self
    }

    /// Set output tokens details.
    pub fn with_output_tokens(mut self, output_tokens: OutputTokens) -> Self {
        self.output_tokens = output_tokens;
        self
    }

    /// Set raw usage data.
    pub fn with_raw(mut self, raw: JSONObject) -> Self {
        self.raw = Some(raw);
        self
    }

    /// Add usage from another Usage (accumulate tokens).
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens.add_assign(&other.input_tokens);
        self.output_tokens.total = Some(
            self.output_tokens
                .total
                .unwrap_or(0)
                .saturating_add(other.output_tokens.total.unwrap_or(0)),
        );
    }
}

/// Breakdown of input (prompt) tokens.
///
/// `total` is normalized to include every input token bucket:
/// `no_cache + cache_read + cache_write` when all three values are known.
/// Provider converters are responsible for adapting provider-specific raw
/// usage shapes before constructing this type.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputTokens {
    /// Total input tokens, including cache-read and cache-write tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    /// Tokens that were not served from cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    no_cache: Option<u64>,
    /// Tokens that were served from cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read: Option<u64>,
    /// Tokens that were written to cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_write: Option<u64>,
}

impl InputTokens {
    /// Build input usage when the provider reports only a normalized total.
    pub fn from_total(total: Option<u64>) -> Self {
        Self {
            total,
            no_cache: None,
            cache_read: None,
            cache_write: None,
        }
    }

    /// Build input usage when every reported input token is uncached.
    pub fn from_uncached(no_cache: Option<u64>) -> Self {
        Self {
            total: no_cache,
            no_cache,
            cache_read: None,
            cache_write: None,
        }
    }

    /// Build input usage from an inclusive provider total plus cache buckets.
    ///
    /// Provider-reported `total` is expected to be ≥ `cache_read + cache_write`.
    /// When the provider violates that (rare but seen in the wild), the
    /// saturating subtraction below pins `no_cache` at 0 instead of underflowing,
    /// and the `debug_assert!` flags the inconsistency in dev builds.
    pub fn from_inclusive_total(
        total: Option<u64>,
        cache_read: Option<u64>,
        cache_write: Option<u64>,
    ) -> Self {
        debug_assert!(
            match (total, cache_read, cache_write) {
                (Some(t), Some(cr), Some(cw)) => t >= cr.saturating_add(cw),
                (Some(t), Some(cr), None) => t >= cr,
                (Some(t), None, Some(cw)) => t >= cw,
                _ => true,
            },
            "provider total={total:?} is less than cache_read={cache_read:?} + cache_write={cache_write:?}",
        );
        let no_cache = total.map(|total| {
            total
                .saturating_sub(cache_read.unwrap_or(0))
                .saturating_sub(cache_write.unwrap_or(0))
        });
        Self {
            total,
            no_cache,
            cache_read,
            cache_write,
        }
    }

    /// Build input usage from exclusive buckets and compute the normalized total.
    pub fn from_exclusive_buckets(
        no_cache: Option<u64>,
        cache_read: Option<u64>,
        cache_write: Option<u64>,
    ) -> Self {
        let total = add_options(add_options(no_cache, cache_read), cache_write);
        Self {
            total,
            no_cache,
            cache_read,
            cache_write,
        }
    }

    pub fn total(&self) -> Option<u64> {
        self.total
    }

    pub fn no_cache(&self) -> Option<u64> {
        self.no_cache
    }

    pub fn cache_read(&self) -> Option<u64> {
        self.cache_read
    }

    pub fn cache_write(&self) -> Option<u64> {
        self.cache_write
    }

    fn add_assign(&mut self, other: &Self) {
        *self = Self {
            total: add_options(self.total, other.total),
            no_cache: add_options(self.no_cache, other.no_cache),
            cache_read: add_options(self.cache_read, other.cache_read),
            cache_write: add_options(self.cache_write, other.cache_write),
        };
    }
}

/// Breakdown of output (completion) tokens.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputTokens {
    /// Total output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Tokens used for text output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<u64>,
    /// Tokens used for reasoning output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
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
