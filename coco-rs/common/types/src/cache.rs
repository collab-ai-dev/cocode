//! Prompt-cache shared types.
//!
//! Provider-neutral data carried through `services/inference` to the
//! Anthropic adapter via `provider_options`. The adapter (`vercel-ai-anthropic`)
//! cannot import this crate directly — it defines structurally-equivalent
//! mirror types and the boundary is JSON. See `docs/coco-rs/prompt-cache-design.md` §7.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;

/// User intent for prompt caching on a request.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    /// No marker emitted, no beta toggled. Default.
    #[default]
    Disabled,
    /// Provider auto-places cache markers per its strategy.
    /// Anthropic: TS-mirror algorithm — last message + per-block system + opt-in tools.
    Auto,
    /// Caller controls placement via `SystemPromptBlock::CacheBreakpoint` hints.
    Manual,
}

/// Cache TTL request. The adapter may downgrade `OneHour` to `FiveMinutes`
/// when eligibility checks fail (TS `should1hCacheTTL`); it never upgrades.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

/// Cache scope. `Global` requires `prompt-caching-scope-2026-01-05` beta
/// and is first-party-only; `Org` is implicit default and not written to wire.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    #[default]
    Org,
    Global,
}

/// Per-call prompt-cache configuration. Forwarded by `services/inference`
/// to the Anthropic adapter; non-Anthropic providers see no caching keys.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PromptCacheConfig {
    pub mode: PromptCacheMode,
    /// Requested TTL; adapter may downgrade per eligibility latch.
    pub ttl: CacheTtl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CacheScope>,
    /// User-requested beta top-up. Adapter merges with capability-derived
    /// betas; mirrors TS `getSdkBetas` input. Adapter applies a TS-mirror
    /// allowlist (`Context1m` only on the typed channel) — see design §10.4.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub requested_betas: BTreeSet<BetaCapability>,
    /// TS `skipCacheWrite` — shifts marker to `messages[N-2]` for fire-and-forget
    /// queries (e.g., title generation) so the main thread's cache prefix
    /// isn't disturbed.
    #[serde(default)]
    pub skip_cache_write: bool,
}

/// Typed enumeration of Anthropic beta capabilities that callers may
/// opt into via `requested_betas`. Adapter translates to wire strings
/// via `beta_capabilities::map_capability`.
///
/// Anthropic-internal experimental gates (`cli-internal-2026-02-09`,
/// `summarize-connector-text-*`) are deliberately NOT enumerated here —
/// coco-rs is an open multi-LLM SDK and does not surface them to public
/// users (design §3.5).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BetaCapability {
    /// Wire name forced to `"context_1m"` (serde's snake_case treats digits
    /// as part of the preceding word and would emit `"context1m"`).
    #[serde(rename = "context_1m")]
    Context1m,
    InterleavedThinking,
    ContextManagement,
    StructuredOutputs,
    TokenEfficientTools,
    FastMode,
    /// Global cache scope; first-party-only.
    PromptCachingScope,
    /// Emitted by adapter; capability gate is `InterleavedThinking + first-party`.
    RedactThinking,
    Advisor,
}

/// Account / billing identity. Drives OAuth beta + 1h-TTL eligibility.
///
/// Bedrock variant intentionally absent — the adapter has no Bedrock
/// endpoint plumbing today (design Non-Goal §2). When Bedrock auth lands,
/// that PR adds back `Bedrock` together with `ProviderTopology::Bedrock`,
/// `bedrock_1h_env`, and the `cache_policy::resolve_ttl` Bedrock branch
/// — all in one PR so a half-implementation is unrepresentable.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// Direct API key (`ANTHROPIC_API_KEY`).
    #[default]
    ApiKey,
    /// OAuth subscriber (Claude.ai login). Drives OAuth beta + 1h-TTL eligibility.
    ClaudeAiSubscriber,
}

#[cfg(test)]
#[path = "cache.test.rs"]
mod tests;
