//! Provider-namespaced options extraction: typed + extras split.
//!
//! Replaces the per-provider hand-rolled `extract_*_options` /
//! `parse_provider_options` patterns with a single canonical entry.
//! Two complementary primitives:
//!
//! - [`ExtractExtras`] — provider-options struct contract: typed
//!   fields PLUS a `#[serde(flatten)] pub extra: BTreeMap<String,
//!   Value>` field. `take_extras` moves the extras out so the caller
//!   can deep-merge them onto the wire body.
//!
//! - [`extract_namespaced`] — looks up the canonical and (optionally)
//!   custom namespace from `ProviderOptions`, deep-merges custom OVER
//!   canonical via [`crate::merge_json_value`], deserializes into the
//!   typed struct, and returns `(typed, extras)`.
//!
//! ## Design contract
//!
//! Per the workspace-level "extra_body overrides typed writes by
//! design" doctrine (see `services/inference/CLAUDE.md` Design Notes):
//!
//! 1. **Custom namespace > canonical namespace** at per-key
//!    deep-merge granularity (e.g. `provider_options["vertex"]`
//!    overrides `provider_options["google"]` for the Vertex Google
//!    adapter, but only on the keys the user actually wrote).
//! 2. **Extras** (whatever `#[serde(flatten)]` captured) are returned
//!    verbatim — the provider's `get_args` deep-merges them onto the
//!    final wire body, where they take final-write priority over typed
//!    body construction.
//!
//! ## TODO(F9): silent-default on bad shape
//!
//! Currently `unwrap_or_default` on bad shape silently drops the
//! entire typed config (a user typo on one key kills every typed
//! field). Replace with a tolerant per-field deser + Warning emission.

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use vercel_ai_provider::ProviderOptions;

use crate::json::merge_json_value;

/// Provider-options struct contract. Every per-provider options type
/// (e.g. `GoogleLanguageModelOptions`, `AnthropicProviderOptions`,
/// `OpenAIChatProviderOptions`) implements this so the catchall
/// `#[serde(flatten)] extra: BTreeMap<String, Value>` field can be
/// extracted by a shared helper.
pub trait ExtractExtras {
    /// Move the catchall extras out of `self`, leaving an empty map.
    fn take_extras(&mut self) -> BTreeMap<String, Value>;
}

/// Look up `canonical_ns` and (when different) `custom_ns` from
/// `provider_options`, deep-merge `custom` OVER `canonical` per-key,
/// deserialize into `T`, and split out the catchall extras.
///
/// Semantics:
/// - `provider_options == None`               → `(default, empty)`
/// - both ns missing                          → `(default, empty)`
/// - canonical-only present                   → typed from canonical
/// - custom-only present                      → typed from custom
/// - both present                             → deep-merge per
///   [`merge_json_value`] (custom wins on per-key overlap), then
///   deserialize
///
/// `canonical_ns == custom_ns` is treated as the single-namespace case
/// (no double lookup).
pub fn extract_namespaced<T>(
    provider_options: Option<&ProviderOptions>,
    canonical_ns: &str,
    custom_ns: &str,
) -> (T, BTreeMap<String, Value>)
where
    T: DeserializeOwned + Default + ExtractExtras,
{
    let Some(opts) = provider_options else {
        return (T::default(), BTreeMap::new());
    };

    let lookup = |ns: &str| -> Value {
        opts.0
            .get(ns)
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or(Value::Null)
    };

    let canonical = lookup(canonical_ns);
    let merged = if custom_ns == canonical_ns {
        canonical
    } else {
        let custom = lookup(custom_ns);
        // canonical = base, custom = overrides (custom wins on overlap)
        merge_json_value(&canonical, &custom)
    };

    if merged.is_null() {
        return (T::default(), BTreeMap::new());
    }

    // TODO(F9): bad-shape silently drops typed config. Replace with
    // tolerant per-field deser + Warning emission.
    let mut typed: T = serde_json::from_value(merged).unwrap_or_default();
    let extras = typed.take_extras();
    (typed, extras)
}

#[cfg(test)]
#[path = "extract_namespaced.test.rs"]
mod tests;
