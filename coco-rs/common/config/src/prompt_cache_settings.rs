//! Prompt-cache + account-identity settings layer.
//!
//! Two sections, deliberately split:
//!
//! - `PromptCacheRuntimeConfig` — provider-agnostic. Today carries the
//!   1h-TTL allowlist; an Anthropic alternative-API or future
//!   caching-aware OpenAI extension can read the same data.
//!
//! - `AccountConfig` — auth/billing identity (`account_kind`,
//!   `in_overage`). Drives 1h-TTL eligibility latch + OAuth beta in
//!   the Anthropic adapter; **session-stable** (R3-F3) — set on
//!   `AnthropicConfig` at provider construction.
//!
//! Anthropic-specific beta-gate knobs (experimental betas,
//! interleaved-thinking disable, show-thinking-summaries,
//! non-interactive) live per-provider-instance under
//! `ProviderConfig.provider_options`; the adapter
//! (`vercel-ai-anthropic`) parses them via `parse_provider_options`.
//!
//! Bridges `~/.coco/settings.json` and `COCO_*` env vars into resolved
//! configs that `services/inference::model_factory::build_anthropic`
//! consumes through plain struct references — no env reads in the
//! adapter or in `services/inference` after `RuntimeConfig` is built.
//!
//! Layering: defaults → settings.json → env overrides → resolved.

use serde::Deserialize;
use serde::Serialize;

use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;

// ── Partial settings (settings.json shape) ──────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialPromptCacheSettings {
    /// 1h-TTL allowlist. Each entry is either an exact match for the
    /// per-call `query_source`, or a `prefix*` glob (single trailing
    /// wildcard).
    pub allowlist: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialAccountSettings {
    /// `"api_key"` or `"claude_ai_subscriber"`. Default = `"api_key"`.
    pub kind: Option<AccountKindSetting>,
    pub in_overage: Option<bool>,
}

/// Wire form for `AccountConfig.kind`. Mirrors `coco_types::AccountKind`
/// — kept in this crate so settings.json doesn't need to import the
/// type from coco-types directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKindSetting {
    #[default]
    ApiKey,
    ClaudeAiSubscriber,
}

// ── Resolved configs ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCacheRuntimeConfig {
    pub allowlist: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    pub kind: coco_types::AccountKind,
    pub in_overage: bool,
}

impl From<AccountKindSetting> for coco_types::AccountKind {
    fn from(v: AccountKindSetting) -> Self {
        match v {
            AccountKindSetting::ApiKey => coco_types::AccountKind::ApiKey,
            AccountKindSetting::ClaudeAiSubscriber => coco_types::AccountKind::ClaudeAiSubscriber,
        }
    }
}

// ── Resolution ───────────────────────────────────────────────────────

impl PromptCacheRuntimeConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        if let Some(list) = settings.prompt_cache.allowlist.as_ref() {
            config.allowlist = list.clone();
        }
        if let Some(raw) = env.get(EnvKey::CocoPromptCacheAllowlist) {
            // Comma-separated list; whitespace-trimmed; empty entries dropped.
            let parsed: Vec<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if !parsed.is_empty() {
                config.allowlist = parsed;
            }
        }
        config
    }
}

impl AccountConfig {
    pub fn resolve(settings: &Settings, _env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        let part = &settings.account;
        if let Some(kind) = part.kind {
            config.kind = kind.into();
        }
        if let Some(v) = part.in_overage {
            config.in_overage = v;
        }
        config
    }
}

#[cfg(test)]
#[path = "prompt_cache_settings.test.rs"]
mod tests;
