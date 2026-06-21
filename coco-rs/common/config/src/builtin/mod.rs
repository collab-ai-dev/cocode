//! Vendor-grouped builtin catalog for providers and models.
//!
//! Each module under `builtin/` owns ONE vendor's data:
//!   * `providers()` — `PartialProviderConfig` entries keyed by
//!     provider name (e.g. `anthropic`, `deepseek-openai`).
//!   * `models()`    — `PartialModelInfo` entries keyed by `model_id`
//!     (e.g. `claude-sonnet-4-6`, `deepseek-v4-flash`).
//!
//! This module aggregates the per-vendor pieces into the public
//! catalog functions consumed by `build_model_registry` and the TUI /
//! CLI startup paths. The aggregators are the *only* code paths that
//! touch all vendors at once; everything else stays vendor-local so
//! adding a new vendor is a single-file change.
//!
//! Aggregation order: `anthropic`, `openai`, `google`, `volcengine`,
//! `zai`, `deepseek`. The provider builder preserves this order so
//! `builtin_providers()` zips byte-stably against
//! `builtin_provider_partials()` (the identity invariant test in
//! `mod.test.rs` relies on the pairing).

mod anthropic;
mod deepseek;
mod google;
mod openai;
mod volcengine;
mod zai;

/// Canonical name of the builtin OpenAI ChatGPT-subscription (OAuth) provider
/// instance. The single source of truth shared by the builtin registry entry
/// (`builtin/openai.rs`) and the `coco login` shorthand resolver
/// (`app/cli::provider_login::instance_name`) — so a rename is one edit the
/// compiler enforces, not two literals that silently drift.
pub const OPENAI_CHATGPT_PROVIDER: &str = "openai-chatgpt";
/// Canonical name of the builtin Gemini Code Assist (OAuth) provider instance.
pub const GEMINI_CODE_ASSIST_PROVIDER: &str = "gemini-code-assist";

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::ConfigError;
use crate::model::ModelInfo;
use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderConfig;

/// Default base instructions used when a `ModelInfo` declares neither
/// `base_instructions` nor `base_instructions_file`. Lives at
/// `instructions/default_prompt.md` (aligned with the claude-code TS
/// source) and is reused by both the runtime SP-fallback path and the
/// DeepSeek builtin catalog.
pub const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../../instructions/default_prompt.md");

/// Compiled-in builtin model registry — well-known models with known
/// metadata. User catalogue files override these per-key.
///
/// Returned as `&'static` partial form so the registry builder can
/// fold it into the same `merge_from` pipeline as the L1 user catalog
/// without re-cloning per (provider, model) pair.
pub fn builtin_models_partial() -> &'static BTreeMap<String, PartialModelInfo> {
    static BUILTIN: OnceLock<BTreeMap<String, PartialModelInfo>> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        let mut m = BTreeMap::new();
        for (id, info) in vendor_models() {
            m.insert(id.to_string(), info);
        }
        m
    })
}

/// Resolved-form view of the builtin registry. Convenience for
/// callers that want a fully validated `ModelInfo` (e.g. UI listings).
pub fn builtin_models_resolved() -> Vec<ModelInfo> {
    builtin_models_partial()
        .iter()
        .filter_map(|(id, p)| ModelInfo::from_partial("__builtin__", id, p.clone()).ok())
        .collect()
}

/// Built-in provider partial overlays. Identity invariant: built
/// through [`ProviderConfig::from_partial`] in [`builtin_providers`]
/// so `name` is set in exactly one code path — the same as
/// user-supplied entries.
pub fn builtin_provider_partials() -> Vec<(&'static str, PartialProviderConfig)> {
    vendor_providers()
}

/// Resolve every builtin partial through `from_partial` so the
/// "name = parent map key" invariant covers builtins as well as
/// user-supplied entries. Returns `ConfigError::IncompleteProviderEntry`
/// if a builtin partial is missing a required field — caught at
/// crate test time.
pub fn builtin_providers() -> Result<Vec<ProviderConfig>, ConfigError> {
    builtin_provider_partials()
        .into_iter()
        .map(|(name, partial)| ProviderConfig::from_partial(name, &partial))
        .collect()
}

fn vendor_models() -> Vec<(&'static str, PartialModelInfo)> {
    let mut out = Vec::new();
    out.extend(anthropic::models());
    out.extend(openai::models());
    out.extend(google::models());
    out.extend(volcengine::models());
    out.extend(zai::models());
    out.extend(deepseek::models());
    out
}

fn vendor_providers() -> Vec<(&'static str, PartialProviderConfig)> {
    let mut out = Vec::new();
    out.extend(anthropic::providers());
    out.extend(openai::providers());
    out.extend(google::providers());
    out.extend(volcengine::providers());
    out.extend(zai::providers());
    out.extend(deepseek::providers());
    out
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
