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
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use vercel_ai_provider::ProviderOptions;

use crate::json::merge_json_value;

/// Result of provider-options extraction.
pub type ExtractNamespacedResult<T> = Result<ExtractedNamespaced<T>, ExtractNamespacedError>;

/// Typed provider options plus raw extras captured by `#[serde(flatten)]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedNamespaced<T> {
    pub typed: T,
    pub extras: BTreeMap<String, Value>,
}

/// Error returned when a present provider-options namespace cannot be
/// deserialized into the provider's typed option schema.
#[derive(Debug, thiserror::Error)]
#[error(
    "invalid provider options for namespace `{namespace}` at {field_path}: {source}; value={value_summary}"
)]
pub struct ExtractNamespacedError {
    pub namespace: String,
    pub field_path: String,
    pub value_summary: String,
    #[source]
    pub source: serde_json::Error,
}

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
) -> ExtractNamespacedResult<T>
where
    T: DeserializeOwned + Default + ExtractExtras,
{
    let Some(opts) = provider_options else {
        return Ok(default_extracted());
    };

    let lookup = |ns: &str| -> Value {
        opts.0
            .get(ns)
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or(Value::Null)
    };

    let canonical = lookup(canonical_ns);
    let (namespace, merged) = if custom_ns == canonical_ns {
        (canonical_ns, canonical)
    } else {
        let custom = lookup(custom_ns);
        // canonical = base, custom = overrides (custom wins on overlap)
        let namespace = if custom.is_null() {
            canonical_ns
        } else {
            custom_ns
        };
        (namespace, merge_json_value(&canonical, &custom))
    };

    if merged.is_null() {
        return Ok(default_extracted());
    }

    let mut typed: T = serde_path_to_error::deserialize(merged.clone()).map_err(|error| {
        let field_path = error.path().to_string();
        let source = error.into_inner();
        ExtractNamespacedError {
            namespace: namespace.to_string(),
            field_path,
            value_summary: summarize_value(&merged),
            source,
        }
    })?;
    let extras = typed.take_extras();
    Ok(ExtractedNamespaced { typed, extras })
}

fn default_extracted<T>() -> ExtractedNamespaced<T>
where
    T: Default,
{
    ExtractedNamespaced {
        typed: T::default(),
        extras: BTreeMap::new(),
    }
}

fn summarize_value(value: &Value) -> String {
    const MAX_CHARS: usize = 240;
    let serialized =
        serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string());
    let mut summary: String = serialized.chars().take(MAX_CHARS).collect();
    if serialized.chars().count() > MAX_CHARS {
        summary.push_str("...");
    }
    summary
}

#[cfg(test)]
#[path = "extract_namespaced.test.rs"]
mod tests;
