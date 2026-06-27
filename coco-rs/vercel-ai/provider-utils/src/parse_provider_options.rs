//! Generic helper for typed-deserializing a provider-namespaced
//! sub-map out of a `ProviderOptions` blob.
//!
//! Mirrors `@ai-sdk/provider-utils::parseProviderOptions`. Where the
//! TS version uses a `FlexibleSchema` (Zod / Valibot / etc), the Rust
//! version uses serde — the caller picks any `DeserializeOwned` type.
//!
//! Replaces the per-provider ad-hoc `extract_*_options` patterns with
//! a single canonical entry. Returns `Ok(None)` when the namespace is
//! missing or empty (the caller should fall through to defaults).
//! Returns `Err` when the namespace is present but doesn't deserialize
//! — that's a user error, not a missing field.
//!
//! Optional providers can pass a fallback namespace (e.g.
//! openai-compatible's `"openaiCompatible"` shared key) via
//! [`parse_provider_options_with_fallback`].
//!
//! Note on type matching: this helper assumes the Rust struct's serde
//! shape matches what the user wrote into ProviderOptions. The
//! workspace convention (`#[serde(rename_all = "camelCase")]` on
//! provider option structs) keeps that aligned with the JS-style
//! camelCase keys most TS docs use.

use serde::de::DeserializeOwned;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ProviderOptions;

/// Look up `provider` in `provider_options` and deserialize to `T`.
///
/// - `provider_options == None` → `Ok(None)`
/// - namespace not present → `Ok(None)`
/// - namespace present but empty map → `Ok(None)` (treated as default)
/// - namespace present + valid → `Ok(Some(parsed))`
/// - namespace present + invalid for `T` → `Err`
pub fn parse_provider_options<T: DeserializeOwned>(
    provider: &str,
    provider_options: Option<&ProviderOptions>,
) -> Result<Option<T>, AISdkError> {
    let Some(opts) = provider_options else {
        return Ok(None);
    };
    let Some(inner) = opts.0.get(provider) else {
        return Ok(None);
    };
    if inner.is_empty() {
        return Ok(None);
    }
    let value = serde_json::to_value(inner).map_err(|e| {
        AISdkError::new(format!(
            "Failed to serialize provider options for '{provider}': {e}"
        ))
    })?;
    serde_json::from_value::<T>(value)
        .map(Some)
        .map_err(|e| AISdkError::new(format!("Invalid '{provider}' provider options: {e}")))
}

/// Try `provider`, then each `fallback` in order, returning the first
/// non-empty namespace successfully deserialized. Used by
/// openai-compatible chat which accepts `<providerName>` AND
/// `openaiCompatible` as accepted ProviderOptions keys.
pub fn parse_provider_options_with_fallback<T: DeserializeOwned>(
    provider: &str,
    fallbacks: &[&str],
    provider_options: Option<&ProviderOptions>,
) -> Result<Option<T>, AISdkError> {
    if let Some(parsed) = parse_provider_options::<T>(provider, provider_options)? {
        return Ok(Some(parsed));
    }
    for fallback in fallbacks {
        if let Some(parsed) = parse_provider_options::<T>(fallback, provider_options)? {
            return Ok(Some(parsed));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "parse_provider_options.test.rs"]
mod tests;
