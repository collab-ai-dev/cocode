//! `ProviderClientFingerprint` — turn-boundary coherence check.
//!
//! Runtime slots carry a fingerprint computed from `RuntimeConfig`; a
//! mismatch on reload means the slot's `Arc<dyn LanguageModelV4>` must be
//! rebuilt.
//!
//! Properties:
//!
//! - **Atomic with role-binding read.** Both `tool_overrides` and
//!   `api_client` are taken from the same `Arc<RuntimeConfig>`
//!   captured at turn start; they cannot diverge.
//! - **Cheap.** Equality compare over a byte-stable struct; no
//!   rebuild when nothing material changed (the common case during
//!   `settings.json` edits that touch only features).
//! - **Key rotation detected.** `api_key_origin_digest` is a SHA-256
//!   over the (env-var-name + resolved-secret) so we never store the
//!   live key. The digest itself is non-reversible.
//! - **`extra_body` is NOT in the fingerprint.** It is per-call
//!   (rebuilt every turn in `build_call_options`); changing it does
//!   not invalidate the cached client.

use std::collections::BTreeMap;

use coco_config::AccountConfig;
use coco_config::PromptCacheRuntimeConfig;
use coco_config::ProviderClientOptions;
use coco_config::ProviderConfig;
use coco_types::ProviderApi;
use coco_types::SubagentRuntimeSnapshot;
use coco_types::WireApi;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;

/// Identity of the live `Arc<dyn LanguageModelV4>` for a (provider,
/// role) pair. Hashable, comparable, and intentionally **does not**
/// include any secret material in cleartext.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderClientFingerprint {
    pub provider: String,
    pub api: ProviderApi,
    pub api_model_name: String,
    pub base_url: String,
    /// Set only for `ProviderApi::Openai` (the lone API where the
    /// model_factory dispatches between `provider.chat()` and
    /// `provider.responses()`). For every other API the field is
    /// inert and would force a needless rebuild on toggle, so it is
    /// excluded from the digest by construction.
    pub wire_api: Option<WireApi>,
    /// SHA-256 over the canonical-JSON serialisation of
    /// `ProviderClientOptions`.
    pub client_options_digest: [u8; 32],
    pub timeout_secs: i64,
    /// SHA-256 over `(env_key_name, resolved_secret_or_empty)`. Detects
    /// rotated keys without storing the secret. Non-reversible.
    pub api_key_origin_digest: [u8; 32],
    /// SHA-256 over the prompt-cache-relevant `RuntimeConfig` sections
    /// (`account`, `prompt_cache`) plus this provider instance's own
    /// `provider_options` map. Mutating any of these via settings
    /// reload invalidates the cached runtime slot so the next turn
    /// picks up the new `AnthropicConfig`. Per-provider scoping
    /// (rather than a workspace-wide knob hash) means a settings
    /// flip on one Anthropic instance doesn't churn an unrelated
    /// instance's client. See `docs/coco-rs/prompt-cache-design.md`
    /// §19.3 attack γ.
    pub runtime_state_digest: [u8; 32],
}

impl ProviderClientFingerprint {
    /// Build a fingerprint for a (provider, model_id) pair without
    /// the runtime-state digest. Equivalent to
    /// [`Self::compute_with_runtime_state`] called with default
    /// (empty) sections. Kept for tests and the
    /// `with_default_fingerprint` mock-client path.
    pub fn compute(provider_cfg: &ProviderConfig, api_model_name: &str) -> Self {
        Self::compute_with_runtime_state(
            provider_cfg,
            api_model_name,
            &AccountConfig::default(),
            &PromptCacheRuntimeConfig::default(),
        )
    }

    /// Build a fingerprint that includes prompt-cache-relevant runtime
    /// state. Call this from `build_api_client` so a settings reload
    /// that flips `account.kind`, `prompt_cache.allowlist`, or any key
    /// inside this provider's `provider_options` invalidates the cached
    /// client at the next turn boundary.
    pub fn compute_with_runtime_state(
        provider_cfg: &ProviderConfig,
        api_model_name: &str,
        account: &AccountConfig,
        prompt_cache: &PromptCacheRuntimeConfig,
    ) -> Self {
        Self {
            provider: provider_cfg.name.clone(),
            api: provider_cfg.api,
            api_model_name: api_model_name.to_string(),
            base_url: provider_cfg.base_url.clone(),
            wire_api: match provider_cfg.api {
                ProviderApi::Openai => Some(provider_cfg.wire_api),
                _ => None,
            },
            client_options_digest: digest_client_options(&provider_cfg.client_options),
            timeout_secs: provider_cfg.timeout_secs,
            api_key_origin_digest: digest_api_key_origin(provider_cfg),
            runtime_state_digest: digest_runtime_state(
                account,
                prompt_cache,
                &provider_cfg.provider_options,
            ),
        }
    }

    /// Project the identity-distinguishing fields onto the cross-layer
    /// `SubagentRuntimeSnapshot` DTO. Drops the SHA-256 digests over
    /// `ProviderClientOptions` and the API-key origin — those are
    /// inference-layer secrets-aware constructs and don't cross into
    /// `coco-types`. The DTO is what `AgentSpawnRequest` carries.
    pub fn to_snapshot(&self) -> SubagentRuntimeSnapshot {
        SubagentRuntimeSnapshot {
            provider: self.provider.clone(),
            api: self.api,
            api_model_name: self.api_model_name.clone(),
            base_url: self.base_url.clone(),
            wire_api: self.wire_api,
        }
    }
}

/// SHA-256 with length-prefixed (be u64 + bytes) field encoding.
///
/// Length prefixing avoids any collision class via delimiter
/// confusion — the inverse mapping is unique. Each field is also
/// preceded by a one-byte tag so reordering produces a different
/// digest.
fn digest_client_options(opts: &ProviderClientOptions) -> [u8; 32] {
    let mut hasher = Sha256::new();
    // headers: tag 0x01, then count, then per-entry (key, mode-tag, raw value).
    // The mode tag distinguishes a literal `"x"` from a template `{template:"x"}`
    // with the same body, and `raw()` hashes the *unexpanded* template so a
    // settings edit to the template string still invalidates the cached client.
    update_u8(&mut hasher, 0x01);
    update_u64(&mut hasher, opts.headers.len() as u64);
    for (k, v) in &opts.headers {
        update_bytes(&mut hasher, k.as_bytes());
        update_u8(&mut hasher, if v.is_templated() { 1 } else { 0 });
        update_bytes(&mut hasher, v.raw().as_bytes());
    }
    update_u8(&mut hasher, 0x02);
    update_optional_bytes(
        &mut hasher,
        opts.auth_token.as_ref().map(|t| t.expose().as_bytes()),
    );
    update_u8(&mut hasher, 0x03);
    update_optional_bytes(
        &mut hasher,
        opts.organization_id.as_deref().map(str::as_bytes),
    );
    update_u8(&mut hasher, 0x04);
    update_optional_bytes(&mut hasher, opts.project_id.as_deref().map(str::as_bytes));
    update_u8(&mut hasher, 0x05);
    update_optional_bool(&mut hasher, opts.include_usage);
    update_u8(&mut hasher, 0x06);
    update_bool(&mut hasher, opts.full_url);
    update_u8(&mut hasher, 0x07);
    update_bool(&mut hasher, opts.supports_structured_outputs);
    hasher.finalize().into()
}

/// SHA-256 over `(env_key_name, resolved_secret_or_marker)` with
/// length-prefixed encoding. Detects rotated keys without storing
/// the secret; the digest is non-reversible.
fn digest_api_key_origin(cfg: &ProviderConfig) -> [u8; 32] {
    let mut hasher = Sha256::new();
    update_u8(&mut hasher, 0x10);
    update_bytes(&mut hasher, cfg.env_key.as_bytes());
    update_u8(&mut hasher, 0x11);
    if let Some(secret) = cfg.resolve_api_key() {
        update_optional_bytes(&mut hasher, Some(secret.as_bytes()));
    } else {
        update_optional_bytes(&mut hasher, None);
    }
    // Auth mode: flipping ApiKey↔OAuth (or changing the OAuth flow) on the
    // same provider name must rebuild the client — an OAuth provider resolves
    // no api_key, so without this the two modes would digest identically when
    // base_url / client_options match.
    update_u8(&mut hasher, 0x12);
    match cfg.auth {
        coco_config::ProviderAuth::ApiKey => update_u8(&mut hasher, 0x00),
        coco_config::ProviderAuth::OAuth { flow } => {
            update_u8(&mut hasher, 0x01);
            update_bytes(&mut hasher, flow.as_str().as_bytes());
        }
    }
    hasher.finalize().into()
}

/// SHA-256 over the prompt-cache-relevant `RuntimeConfig` sections
/// (`account` + `prompt_cache`) plus this provider's own
/// `provider_options` map. Length-prefixed, tagged-per-field, identical
/// pattern to `digest_client_options`.
///
/// Mutating any of these via settings reload changes the digest, so
/// the next turn-boundary fingerprint compare in `QueryEngine` rebuilds
/// the cached `Arc<dyn LanguageModelV4>` and the new provider config
/// propagates without a process restart.
///
/// `provider_options` is the per-instance opaque knob map (see
/// `ProviderConfig.provider_options`). Hashing it here scopes
/// invalidation to the affected provider — a flip on one Anthropic
/// instance won't churn an unrelated instance's client. The map is
/// already a `BTreeMap`, so iteration order is deterministic; values
/// are serialized via canonical JSON so structurally-equal trees
/// produce byte-equal input.
fn digest_runtime_state(
    account: &AccountConfig,
    prompt_cache: &PromptCacheRuntimeConfig,
    provider_options: &BTreeMap<String, Value>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    // account.kind: tag 0x20, 1-byte discriminant.
    update_u8(&mut hasher, 0x20);
    let kind_tag: u8 = match account.kind {
        coco_types::AccountKind::ApiKey => 0x00,
        coco_types::AccountKind::ClaudeAiSubscriber => 0x01,
    };
    update_u8(&mut hasher, kind_tag);
    // account.in_overage: tag 0x21.
    update_u8(&mut hasher, 0x21);
    update_bool(&mut hasher, account.in_overage);
    // prompt_cache.allowlist: tag 0x22, count, then per-entry bytes.
    update_u8(&mut hasher, 0x22);
    update_u64(&mut hasher, prompt_cache.allowlist.len() as u64);
    for pat in &prompt_cache.allowlist {
        update_bytes(&mut hasher, pat.as_bytes());
    }
    // provider_options: tag 0x30, count, then per-entry (key bytes,
    // canonical-JSON value bytes). `BTreeMap` iteration is sorted, and
    // `serde_json::to_vec` on a `Value` is deterministic (object keys
    // are emitted in insertion order, but we only ever store
    // `BTreeMap`-derived values here so the order matches).
    update_u8(&mut hasher, 0x30);
    update_u64(&mut hasher, provider_options.len() as u64);
    for (k, v) in provider_options {
        update_bytes(&mut hasher, k.as_bytes());
        let bytes = serde_json::to_vec(v).unwrap_or_default();
        update_bytes(&mut hasher, &bytes);
    }
    hasher.finalize().into()
}

fn update_u8(h: &mut Sha256, b: u8) {
    h.update([b]);
}

fn update_u64(h: &mut Sha256, n: u64) {
    h.update(n.to_be_bytes());
}

fn update_bytes(h: &mut Sha256, bytes: &[u8]) {
    update_u64(h, bytes.len() as u64);
    h.update(bytes);
}

fn update_optional_bytes(h: &mut Sha256, value: Option<&[u8]>) {
    match value {
        Some(b) => {
            update_u8(h, 0x01);
            update_bytes(h, b);
        }
        None => update_u8(h, 0x00),
    }
}

fn update_bool(h: &mut Sha256, b: bool) {
    update_u8(h, if b { 0x01 } else { 0x00 });
}

fn update_optional_bool(h: &mut Sha256, value: Option<bool>) {
    match value {
        Some(true) => update_u8(h, 0x02),
        Some(false) => update_u8(h, 0x01),
        None => update_u8(h, 0x00),
    }
}

#[cfg(test)]
#[path = "fingerprint.test.rs"]
mod tests;
