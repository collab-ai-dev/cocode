//! `PromptCacheConfig` → flat camelCase `extra_body` keys.
//!
//! Pure pass-through emission of the user's prompt-cache directive
//! into `provider_options[<namespace>]`. **No policy interpretation**
//! happens here — `services/inference/CLAUDE.md` requires Anthropic
//! prompt-cache + beta policy to live in `vercel-ai-anthropic`. This
//! module is the inference-side mirror of `thinking_convert`.
//!
//! Output is a `BTreeMap<String, JSONValue>` with camelCase keys to
//! match the adapter's `#[serde(rename_all = "camelCase")]`. Layer 2
//! (`build_call_options`) merges this into the per-call `extra_body`
//! and wraps under the SDK namespace key.
//!
//! Non-Anthropic providers receive an empty map; no namespace pollution.
//!
//! Session-stable account / billing identity (`AccountKind`,
//! `in_overage`) is **NOT** emitted here — those values reach the
//! Anthropic adapter via `AnthropicConfig` (set by `build_anthropic`
//! from `RuntimeConfig.account.*` at provider construction). See
//! `docs/coco-rs/prompt-cache-design.md` §9.2 / R3-F3.

use coco_types::PromptCacheConfig;
use coco_types::PromptCacheMode;
use coco_types::ProviderApi;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

/// Pass-through emission of `cacheStrategy` + `requestedBetas`.
/// Returns an empty map for non-Anthropic providers.
pub fn to_extra_body(cfg: &PromptCacheConfig, api: ProviderApi) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if cfg.mode == PromptCacheMode::Disabled || !matches!(api, ProviderApi::Anthropic) {
        return out;
    }
    out.insert(
        "cacheStrategy".into(),
        json!({
            "mode": cfg.mode,
            "ttl": cfg.ttl,
            "scope": cfg.scope,
            "skipCacheWrite": cfg.skip_cache_write,
        }),
    );
    if !cfg.requested_betas.is_empty() {
        out.insert(
            "requestedBetas".into(),
            serde_json::to_value(&cfg.requested_betas).unwrap_or(Value::Null),
        );
    }
    out
}

/// Pass-through emission of per-call session context. The adapter
/// consumes these as opaque data and applies its own policy.
///
/// **Gated on a non-disabled cache strategy** (design §9.2 / Finding 4).
/// Without this gate, `query_source` would re-hash `extra_body_hash`
/// for callers that never enabled caching, breaking the
/// `query_source_change_does_NOT_change_hash_when_strategy_disabled`
/// test. Session context is load-bearing **only when caching is on**.
pub fn session_context_to_extra_body(
    cache_cfg: Option<&PromptCacheConfig>,
    agentic: bool,
    query_source: Option<&str>,
    api: ProviderApi,
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if !matches!(api, ProviderApi::Anthropic) {
        return out;
    }
    let active = matches!(cache_cfg, Some(c) if c.mode != PromptCacheMode::Disabled);
    if !active {
        return out;
    }
    out.insert("agenticQuery".into(), Value::Bool(agentic));
    if let Some(qs) = query_source {
        out.insert("querySource".into(), Value::String(qs.into()));
    }
    out
}

#[cfg(test)]
#[path = "cache_convert.test.rs"]
mod tests;
