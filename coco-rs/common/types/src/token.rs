use serde::Deserialize;
use serde::Serialize;

/// Input-side token breakdown.
///
/// Shape mirrors `vercel_ai_provider::InputTokens` — `total` is the
/// normalized count and equals `no_cache + cache_read + cache_write`
/// when the provider reports every bucket. Provider converters in
/// `services/inference` are responsible for normalizing per-provider
/// raw shapes (Anthropic exclusive-bucket vs OpenAI inclusive-total)
/// before populating this struct, so consumers can rely on `total`
/// being the post-cache-aware true input count.
///
/// `i64` is used in place of vercel-ai's `Option<u64>` to match the
/// rest of coco-rs's token-count idiom; "not reported" surfaces as `0`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTokens {
    /// Total input tokens (includes cache reads + cache writes).
    #[serde(default)]
    pub total: i64,
    /// Input tokens that did NOT come from the prompt cache.
    #[serde(default)]
    pub no_cache: i64,
    /// Input tokens served from the prompt cache. Anthropic
    /// `cache_read_input_tokens` / OpenAI `prompt_tokens_details.cached_tokens`
    /// / Google `cachedContentTokenCount` all map here.
    #[serde(default)]
    pub cache_read: i64,
    /// Input tokens written to the prompt cache. Only Anthropic's
    /// `cache_creation_input_tokens` populates this; OpenAI-compatible
    /// providers have no wire equivalent and the value stays 0.
    #[serde(default)]
    pub cache_write: i64,
}

/// Output-side token breakdown.
///
/// Shape mirrors `vercel_ai_provider::OutputTokens` — `total` is the
/// total emitted, and `text + reasoning` decompose it when the provider
/// reports the breakdown.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTokens {
    /// Total output tokens.
    #[serde(default)]
    pub total: i64,
    /// Output tokens spent on plain text (non-reasoning).
    #[serde(default)]
    pub text: i64,
    /// Output tokens spent on reasoning / thinking blocks. Already
    /// counted inside `total` and billed at the output rate.
    #[serde(default)]
    pub reasoning: i64,
}

/// Per-request token counts (returned by LLM API).
///
/// Shape mirrors `vercel_ai_provider::Usage` — nested input/output
/// breakdown with named cache buckets, so `usage.input_tokens.total`
/// is unambiguously the normalized count and the implicit "is this
/// the no-cache subset or the inclusive total?" contract that the
/// previous flat shape carried is gone.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input-side breakdown.
    #[serde(default)]
    pub input_tokens: InputTokens,
    /// Output-side breakdown.
    #[serde(default)]
    pub output_tokens: OutputTokens,
}

/// Persisted and protocol-visible cumulative usage for one session.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageSnapshot {
    pub version: i32,
    pub session_id: String,
    pub updated_at_ms: i64,
    pub totals: SessionUsageTotals,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<SessionModelUsageEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpriced_models: Vec<crate::ProviderModelSelection>,
}

/// Session-level token and cost totals.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub web_search_requests: i64,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub total_cost_usd: f64,
    pub request_count: i64,
    #[serde(default)]
    pub unpriced_request_count: i64,
    #[serde(default)]
    pub unpriced_input_tokens: i64,
    #[serde(default)]
    pub unpriced_output_tokens: i64,
}

/// Cumulative usage for a single `(provider, model_id)` bucket.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionModelUsageEntry {
    pub provider: String,
    pub model_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub web_search_requests: i64,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub total_cost_usd: f64,
    pub request_count: i64,
    #[serde(default)]
    pub unpriced_request_count: i64,
    #[serde(default)]
    pub unpriced_input_tokens: i64,
    #[serde(default)]
    pub unpriced_output_tokens: i64,
    /// True only when every request in this bucket had known pricing.
    pub priced: bool,
}

impl TokenUsage {
    /// `input_tokens.total + output_tokens.total`. Use this when totalling
    /// across calls.
    pub fn total(&self) -> i64 {
        self.input_tokens
            .total
            .saturating_add(self.output_tokens.total)
    }
}

impl std::ops::Add for TokenUsage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: InputTokens {
                total: self
                    .input_tokens
                    .total
                    .saturating_add(rhs.input_tokens.total),
                no_cache: self
                    .input_tokens
                    .no_cache
                    .saturating_add(rhs.input_tokens.no_cache),
                cache_read: self
                    .input_tokens
                    .cache_read
                    .saturating_add(rhs.input_tokens.cache_read),
                cache_write: self
                    .input_tokens
                    .cache_write
                    .saturating_add(rhs.input_tokens.cache_write),
            },
            output_tokens: OutputTokens {
                total: self
                    .output_tokens
                    .total
                    .saturating_add(rhs.output_tokens.total),
                text: self
                    .output_tokens
                    .text
                    .saturating_add(rhs.output_tokens.text),
                reasoning: self
                    .output_tokens
                    .reasoning
                    .saturating_add(rhs.output_tokens.reasoning),
            },
        }
    }
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// Per-model accumulated usage (for cost tracking in coco-messages).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub web_search_requests: i64,
    pub cost_usd: f64,
}

impl ModelUsage {
    pub fn accumulate(&mut self, usage: TokenUsage, cost: f64) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens.total);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens.total);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(usage.input_tokens.cache_read);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(usage.input_tokens.cache_write);
        self.cost_usd += cost;
    }
}
