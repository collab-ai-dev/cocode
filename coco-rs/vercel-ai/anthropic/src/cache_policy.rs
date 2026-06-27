//! Cache policy: 1h-TTL eligibility latch + per-call allowlist match.
//!
//! Two distinct latches:
//!
//! - **Eligibility (`OnceLock<bool>`):** session-stable. ApiKey accounts
//!   are always eligible; subscriber accounts are eligible only when
//!   `in_overage` is true. Computed once on first call and frozen for
//!   the session — flipping `in_overage` mid-session needs a session
//!   reload (which rebuilds `AnthropicConfig` and replaces this policy
//!   instance with a fresh latch).
//!
//! - **Allowlist (`OnceLock<Vec<String>>`):** session-stable snapshot
//!   of the allowlist patterns at first call. The per-call match
//!   (exact / `prefix*` glob) is recomputed every call against this
//!   snapshot — a new `query_source` doesn't need an adapter rebuild
//!   to be matched against the existing patterns.
//!
//! The eligibility latch is the surprising part: TS deliberately
//! freezes the value after first observation so a mid-session billing
//! flip can't silently start charging higher per-token rates. We
//! preserve that property explicitly here. Design §10.2 / R3-F3.

use std::sync::OnceLock;

use crate::anthropic_config::AdapterAccountKind;
use crate::anthropic_config::AnthropicConfig;
use crate::messages::anthropic_messages_options::AdapterCacheTtl;
use crate::messages::anthropic_messages_options::CacheStrategy;

/// Session-stable cache policy. Lives on a single `AnthropicConfig`
/// instance; both latches are populated lazily on first call.
#[derive(Debug, Default)]
pub struct CachePolicy {
    eligible_1h: OnceLock<bool>,
    allowlist: OnceLock<Vec<String>>,
}

impl CachePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the effective TTL for this call. Caller-requested TTL is
    /// honored only if the (account, allowlist) pair admits it;
    /// otherwise downgrades to 5m. `None` returned when the strategy is
    /// `Disabled` — caller skips cache markers entirely.
    ///
    /// Pure aside from the `OnceLock::get_or_init` writes (idempotent
    /// after first call). Safe to call from concurrent turns on the
    /// same config.
    pub fn resolve_ttl(
        &self,
        config: &AnthropicConfig,
        strategy: &CacheStrategy,
        query_source: Option<&str>,
    ) -> Option<AdapterCacheTtl> {
        if matches!(
            strategy.mode,
            crate::messages::anthropic_messages_options::AdapterCacheMode::Disabled
        ) {
            return None;
        }
        // Caller asked for 5m → no eligibility check needed.
        if matches!(strategy.ttl, AdapterCacheTtl::FiveMinutes) {
            return Some(AdapterCacheTtl::FiveMinutes);
        }

        // 1h requested. Two gates: account eligibility (session-latched)
        // AND allowlist match (per-call against latched snapshot).
        let eligible = *self.eligible_1h.get_or_init(|| compute_eligibility(config));
        if !eligible {
            return Some(AdapterCacheTtl::FiveMinutes);
        }
        let allowlist = self
            .allowlist
            .get_or_init(|| config.prompt_cache_allowlist.clone());
        if matches_allowlist(allowlist, query_source) {
            Some(AdapterCacheTtl::OneHour)
        } else {
            Some(AdapterCacheTtl::FiveMinutes)
        }
    }
}

/// Eligibility rule (TS `should1hCacheTTL`):
/// - ApiKey accounts: always eligible.
/// - ClaudeAiSubscriber: eligible only when `in_overage` is true.
fn compute_eligibility(config: &AnthropicConfig) -> bool {
    match config.account_kind {
        AdapterAccountKind::ApiKey => true,
        AdapterAccountKind::ClaudeAiSubscriber => config.in_overage,
    }
}

/// Allowlist match: exact, or `prefix*` glob (single trailing wildcard).
/// Empty allowlist + non-empty query_source → no match (intentional —
/// users opt-in per query source). Missing query_source → no match.
fn matches_allowlist(allowlist: &[String], query_source: Option<&str>) -> bool {
    let Some(qs) = query_source else {
        return false;
    };
    for pat in allowlist {
        if let Some(prefix) = pat.strip_suffix('*') {
            if qs.starts_with(prefix) {
                return true;
            }
        } else if pat == qs {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "cache_policy.test.rs"]
mod tests;
