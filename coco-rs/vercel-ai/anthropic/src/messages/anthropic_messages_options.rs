use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider_utils::ExtractExtras;
use vercel_ai_provider_utils::extract_namespaced;

/// Anthropic-specific thinking configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ThinkingConfig {
    /// For Sonnet 4.6, Opus 4.6, and newer models.
    #[serde(rename = "adaptive")]
    Adaptive,
    /// For models before Opus 4.6 (except Sonnet 4.6 which still supports it).
    #[serde(rename = "enabled")]
    Enabled {
        #[serde(rename = "budgetTokens")]
        budget_tokens: Option<u64>,
    },
    /// Disable thinking.
    #[serde(rename = "disabled")]
    Disabled,
}

/// Structured output mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StructuredOutputMode {
    OutputFormat,
    JsonTool,
    Auto,
}

/// Effort level.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// Speed mode (Opus 4.6 only).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fast,
    Standard,
}

impl Speed {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Standard => "standard",
        }
    }
}

/// MCP server configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub name: String,
    pub url: String,
    pub authorization_token: Option<String>,
    pub tool_configuration: Option<McpToolConfiguration>,
}

/// MCP tool configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolConfiguration {
    pub enabled: Option<bool>,
    pub allowed_tools: Option<Vec<String>>,
}

/// Container configuration for agent skills.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerConfig {
    pub id: Option<String>,
    pub skills: Option<Vec<ContainerSkill>>,
}

/// A skill in a container.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerSkill {
    #[serde(rename = "type")]
    pub skill_type: String,
    pub skill_id: String,
    pub version: Option<String>,
}

/// Cache control configuration.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct CacheControlConfig {
    #[serde(rename = "type")]
    pub cache_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

/// Adapter-side mirror of `coco_types::PromptCacheMode`. Same wire shape;
/// no shared type (`vercel-ai-anthropic` cannot import `coco-types` per
/// the L0 layer rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheMode {
    Disabled,
    Auto,
    Manual,
}

/// Adapter-side mirror of `coco_types::CacheTtl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheTtl {
    FiveMinutes,
    OneHour,
}

/// Adapter-side mirror of `coco_types::CacheScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheScope {
    Org,
    Global,
}

/// Adapter-side mirror of `coco_types::BetaCapability`.
///
/// Wire boundary: snake_case (matches what
/// `services/inference::cache_convert` writes). The adapter's
/// `beta_capabilities::map_capability` then translates each enum variant
/// into the actual Anthropic header string (kebab-case + date suffix,
/// e.g. `"context-1m-2025-08-07"`). Two distinct hops:
/// JSON-snake → Rust enum → Anthropic-kebab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterBetaCapability {
    /// Wire name forced to `"context_1m"` to match the upstream coco-types
    /// rename — serde's snake_case treats digits as part of the preceding
    /// word and would emit `"context1m"`.
    #[serde(rename = "context_1m")]
    Context1m,
    InterleavedThinking,
    ContextManagement,
    StructuredOutputs,
    TokenEfficientTools,
    FastMode,
    PromptCachingScope,
    RedactThinking,
    Advisor,
    /// Server-side `tool_reference` expansion + `defer_loading` on
    /// tool definitions (`tool-search-tool-2025-10-19`). Emitted when
    /// `AnthropicModelCapabilities.tool_reference` is true and the
    /// request actually carries deferred tools.
    ToolSearch,
}

/// Per-call cache strategy directive. Mirror of `coco_types::PromptCacheConfig`
/// without the `requested_betas` field (which lives at the
/// `AnthropicProviderOptions` top level).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStrategy {
    pub mode: AdapterCacheMode,
    /// Caller-requested TTL. Adapter may downgrade based on eligibility.
    pub ttl: AdapterCacheTtl,
    #[serde(default)]
    pub scope: Option<AdapterCacheScope>,
    #[serde(default)]
    pub skip_cache_write: bool,
}

/// Anthropic-specific provider options.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicProviderOptions {
    /// Whether to send reasoning to the model.
    pub send_reasoning: Option<bool>,
    /// Structured output mode.
    pub structured_output_mode: Option<StructuredOutputMode>,
    /// Extended thinking configuration.
    pub thinking: Option<ThinkingConfig>,
    /// Disable parallel tool use.
    pub disable_parallel_tool_use: Option<bool>,
    /// Cache control settings (low-level user override; composes
    /// additively with auto-placed markers).
    pub cache_control: Option<CacheControlConfig>,
    /// MCP servers.
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    /// Container/agent skills configuration.
    pub container: Option<ContainerConfig>,
    /// Enable/disable tool streaming.
    pub tool_streaming: Option<bool>,
    /// Effort level.
    pub effort: Option<Effort>,
    /// Speed mode (Opus 4.6 only).
    pub speed: Option<Speed>,
    /// Extra beta features.
    pub anthropic_beta: Option<Vec<String>>,
    /// Context management configuration.
    pub context_management: Option<Value>,
    /// Inference geography constraint (`"us"` or `"global"`).
    pub inference_geo: Option<String>,

    // ─── Prompt-cache per-call fields (design §10.1) ─────────────────
    /// High-level cache strategy: auto-place markers, choose TTL/scope.
    /// Adapter resolves to wire `cache_control` blocks via
    /// `cache_placement::compute_marker_index_post_group`.
    pub cache_strategy: Option<CacheStrategy>,
    /// User-requested beta top-up (TS `getSdkBetas` equivalent).
    /// Adapter applies a TS-mirror allowlist (`Context1m` only on the
    /// typed channel) — design §10.4 / Finding F4.
    pub requested_betas: Option<Vec<AdapterBetaCapability>>,
    /// Per-call agentic flag — gates the `claude-code-20250219`
    /// baseline beta. Helper calls (compaction, title generation)
    /// pass `false`; main agent loop passes `true`.
    pub agentic_query: Option<bool>,
    /// Query source — matched against the 1h-TTL allowlist per call.
    pub query_source: Option<String>,
    // **Round-3 Finding 3:** `account_kind` and `in_overage` are NOT
    // per-call fields. They live on `AnthropicConfig` (session-stable)
    // because `cache_policy::resolve_ttl` latches eligibility on the
    // first call — a missing first-call value would default-corrupt
    // the latch for the whole session.

    // Catches every key not consumed by the typed fields above. The
    // language model deep-merges this onto the wire body via
    // `merge_json_value`, so callers (`coco_inference::thinking_convert`,
    // user extras) can push arbitrary extra_body fields — including
    // nested paths — without code changes. Typed-consumed keys (e.g.
    // `cacheStrategy`, `requestedBetas`, `agenticQuery`, `querySource`)
    // are now bound to typed fields and so never leak into wire body
    // root via extras.
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ExtractExtras for AnthropicProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, Value> {
        std::mem::take(&mut self.extra)
    }
}

/// Extract Anthropic-specific options from generic provider options.
///
/// Returns `(typed, raw)`:
///
/// - `typed` — parsed `AnthropicProviderOptions`, used for header /
///   beta / validation side-effects (e.g. `interleaved-thinking-*`
///   beta, MCP tool validation, `code-execution-*` beta gates).
/// - `raw` — extras captured by `#[serde(flatten)]`, deep-merged into
///   the wire body root at the end of `get_args` via `merge_json_value`.
///   **Opaque to coco-rs** — users are responsible for the correctness
///   of their keys and shapes.
///
/// Namespace policy: parses canonical `"anthropic"` and a custom
/// provider-name namespace (e.g. `"my-proxy"`, derived from
/// `"my-proxy.messages"` by stripping the suffix), **deep-merging**
/// them with custom winning on per-key overlap (delegated to
/// `vercel_ai_provider_utils::extract_namespaced`). Replaces the
/// previous hand-written per-`Option<T>` `.or()` chain — for nested
/// struct fields the new path is more correct (per-key merge rather
/// than whole-struct replace).
pub fn extract_anthropic_options(
    provider_options: &Option<ProviderOptions>,
    provider: &str,
) -> (AnthropicProviderOptions, BTreeMap<String, Value>) {
    // Extract custom provider name prefix (e.g., "my-proxy.messages" → "my-proxy")
    let provider_name = match provider.find('.') {
        Some(idx) => &provider[..idx],
        None => provider,
    };

    extract_namespaced(provider_options.as_ref(), "anthropic", provider_name)
}

#[cfg(test)]
#[path = "anthropic_messages_options.test.rs"]
mod tests;
