//! Per-provider rate-limit state observed from inference responses.
//!
//! Lives on [`ToolAppState::rate_limits`](crate::ToolAppState), keyed by
//! the provider **instance name** (matches
//! `services/inference::ProviderClientFingerprint::provider`, NOT the
//! `ProviderApi` discriminator — two `OpenaiCompat` instances "groq" and
//! "together" coexist independently).
//!
//! Multi-provider design: TS upstream's `getSuggestionSuppressReason` is
//! Anthropic-only via `claudeAiLimits.ts`. coco-rs supports 6+ providers,
//! so the suppression decision must be **selective** — only the fork's
//! actual provider matters. The selectivity key is `cache.provider`
//! (recorded on [`crate::CacheSafeParams`]), which respects fast-mode
//! swaps because it captures the literally-active provider.
//!
//! Stale entries are pruned by the engine on each finalize_turn so the
//! map stays bounded by the number of configured providers.

use crate::ProviderApi;
use crate::event::RateLimitStatus;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RateLimitEntry {
    pub api: ProviderApi,
    pub status: RateLimitStatus,
    /// Wall-clock unix-ms when the limit window resets. `None` when the
    /// provider didn't surface a `*-reset` header. The engine prunes
    /// entries whose `reset_at_ms` has passed; `None` entries persist
    /// until a successful call overwrites them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at_ms: Option<i64>,
    /// Raw `Retry-After` header value (seconds), kept for telemetry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<i64>,
    pub last_observed_ms: i64,
}

#[cfg(test)]
#[path = "rate_limit.test.rs"]
mod tests;
