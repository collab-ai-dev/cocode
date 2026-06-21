use std::collections::HashMap;
use std::sync::Arc;

/// Adapter-local mirror of `coco_types::AccountKind` (`vercel-ai-anthropic`
/// is L0 and cannot import `coco-*`). Wire JSON is identical so the
/// boundary stays parity-checked by `wire_round_trip` tests.
///
/// Bedrock variant intentionally absent — the adapter has no Bedrock
/// endpoint plumbing today (design Non-Goal §2). When Bedrock auth lands,
/// that PR adds back `Bedrock` together with `ProviderTopology::Bedrock`,
/// `bedrock_1h_env`, and the `cache_policy::resolve_ttl` Bedrock branch
/// — all in one PR so a half-implementation is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterAccountKind {
    /// Direct API key (`ANTHROPIC_API_KEY`).
    #[default]
    ApiKey,
    /// OAuth subscriber (Claude.ai login). Drives OAuth beta + 1h-TTL eligibility.
    ClaudeAiSubscriber,
}

/// Adapter-local mirror of `coco_types::Capability` translated into a
/// bool-per-feature struct. The provider factory in
/// `services/inference::model_factory` writes this from
/// `ResolvedModel.info.capabilities`. All-false = "unknown model" safe
/// default — no capability betas emitted, no auto cache marker.
///
/// Bool-per-feature beats a Vec of strings (no string matching) and
/// beats a parallel enum (no duplicated taxonomy when only 5 boolean
/// toggles are needed). Same shape as the existing
/// `supports_native_structured_output: Option<bool>` pattern below.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnthropicModelCapabilities {
    pub prompt_cache: bool,
    pub context_1m: bool,
    pub interleaved_thinking: bool,
    pub context_management: bool,
    pub token_efficient_tools: bool,
    /// Server expands `tool_reference` content blocks into
    /// `<functions>...</functions>` markup before the prompt reaches
    /// the model. When true, the adapter emits `defer_loading: true`
    /// on tools the caller marks for deferral and adds the
    /// `tool-search-tool-2025-10-19` beta header. Lets the caller keep
    /// the `tools` array constant across turns (cache-friendly).
    ///
    /// Maps from `coco_types::Capability::ServerSideToolReference`.
    pub tool_reference: bool,
}

/// Endpoint family — currently single-variant by design (Bedrock /
/// Foundry / Vertex / proxy support all deferred — see design Non-Goal §2).
/// The enum is kept (not collapsed to a `bool is_first_party`) so a
/// future Bedrock PR adds a variant without touching every gate site
/// — `matches!(topology, ProviderTopology::FirstParty)` predicates
/// already in place stay correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderTopology {
    /// `api.anthropic.com` (firstParty). Gets all firstParty-only betas
    /// (`redact-thinking-2026-02-12`, `prompt-caching-scope-2026-01-05`,
    /// `context-management-2025-06-27`) and global cache scope.
    #[default]
    FirstParty,
}

/// Shared configuration passed to each Anthropic model instance.
pub struct AnthropicConfig {
    /// Provider identifier (e.g., "anthropic.messages").
    pub provider: String,
    /// Base URL for the API (e.g., "https://api.anthropic.com/v1").
    pub base_url: String,
    /// Lazy header supplier — called per-request to get auth + custom headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
    /// When false, the model will use JSON tool fallback for structured outputs.
    /// Defaults to true.
    pub supports_native_structured_output: Option<bool>,
    /// When false, `strict` on tool definitions will be ignored and a warning emitted.
    /// Defaults to true.
    pub supports_strict_tools: Option<bool>,
    /// When `true`, `base_url` is the complete endpoint URL — no API path
    /// suffix is appended. Default (`None`): auto-detect duplicate suffixes.
    pub full_url: Option<bool>,

    // ─── Prompt-cache + beta-policy fields (design §10.0) ────────────
    /// Resolved per-model capability bools. Set by the provider factory
    /// (`services/inference::model_factory::build_anthropic`) from
    /// `ResolvedModel.info.capabilities`. All-false = unknown-model
    /// safe default.
    pub capabilities: AnthropicModelCapabilities,

    /// Endpoint topology — distinct from auth (`AdapterAccountKind`).
    /// Drives `shouldIncludeFirstPartyOnlyBetas` (FirstParty only) and
    /// `shouldUseGlobalCacheScope` (FirstParty only).
    pub provider_topology: ProviderTopology,

    /// TS `!DISABLE_EXPERIMENTAL_BETAS` (`betas.ts:215`). Drives
    /// first-party-only beta inclusion (RedactThinking,
    /// PromptCachingScope, ContextManagement). Default true.
    pub experimental_betas_enabled: bool,

    /// TS `process.env.DISABLE_INTERLEAVED_THINKING` (`betas.ts:258-262`).
    /// Suppresses `interleaved-thinking-2025-05-14` even on capable models.
    pub disable_interleaved_thinking: bool,

    /// TS `getInitialSettings().showThinkingSummaries` (`betas.ts:268-275`).
    /// Suppresses `redact-thinking-2026-02-12` when true.
    pub show_thinking_summaries: bool,

    /// TS `getIsNonInteractiveSession()` (`betas.ts:268-275`). Suppresses
    /// `redact-thinking-2026-02-12` for non-interactive runs.
    pub non_interactive: bool,

    /// 1h-TTL allowlist patterns. Each entry is either an exact match for
    /// `query_source`, or a `prefix*` glob. Source: settings.json
    /// `prompt_cache.allowlist` (TS reads `tengu_prompt_cache_1h_config`
    /// from GrowthBook).
    pub prompt_cache_allowlist: Vec<String>,

    /// **Session-stable** account / billing identity (R3-F3). Sourced
    /// from `RuntimeConfig.account.kind` via `build_anthropic`. MUST
    /// live on the session-stable config — NOT on per-call
    /// `AnthropicProviderOptions` — because `cache_policy::resolve_ttl`
    /// latches eligibility on first call and a missing first-call value
    /// would silently corrupt every later subscriber request for the
    /// lifetime of this language model.
    pub account_kind: AdapterAccountKind,

    /// **Session-stable** subscriber overage flag (R3-F3). Same reasoning
    /// as `account_kind`. When the user's overage status flips
    /// mid-session, the session reload path rebuilds `RuntimeConfig`
    /// and the next provider construction picks up the new value; the
    /// in-flight `OnceLock` keeps the pre-flip latch.
    pub in_overage: bool,
}

impl AnthropicConfig {
    /// Build a full URL from a path segment (e.g., "/messages").
    ///
    /// If `full_url` is set, or `base_url` already ends with the path,
    /// returns `base_url` as-is to avoid duplication.
    pub fn url(&self, path: &str) -> String {
        if self.full_url.unwrap_or(false) || self.base_url.ends_with(path) {
            self.base_url.clone()
        } else {
            format!("{}{path}", self.base_url)
        }
    }

    /// Get the current headers by invoking the lazy supplier.
    pub fn get_headers(&self) -> HashMap<String, String> {
        (self.headers)()
    }
}
