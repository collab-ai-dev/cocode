//! Utilities for resolving provider options keys.
//!
//! OpenAI-compatible providers are often constructed with hyphenated names
//! (e.g. `"my-provider"`). Users can pass provider options under either
//! the raw name or its camelCase equivalent (`"myProvider"`).  This module
//! helps normalise the lookup and emit deprecation warnings when the raw
//! (non-camelCase) key is used.

use vercel_ai_provider::JSONValue;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::Warning;

type ProviderOptionMap = std::collections::HashMap<String, JSONValue>;

/// Convert a hyphenated or underscored string to camelCase.
///
/// ```
/// use vercel_ai_openai_compatible::provider_options_key::to_camel_case;
/// assert_eq!(to_camel_case("my-provider"), "myProvider");
/// assert_eq!(to_camel_case("my_provider"), "myProvider");
/// assert_eq!(to_camel_case("already"), "already");
/// ```
pub fn to_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '-' || ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            out.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Return the provider options object for an OpenAI-compatible provider.
///
/// Precedence: camelCase provider key, raw provider key,
/// then the shared `openaiCompatible` fallback.
pub fn get_effective_provider_options<'a>(
    raw_name: &str,
    provider_options: Option<&'a ProviderOptions>,
) -> Option<&'a ProviderOptionMap> {
    let opts = provider_options?;
    let camel = to_camel_case(raw_name);
    opts.0
        .get(&camel)
        .or_else(|| opts.0.get(raw_name))
        .or_else(|| opts.0.get("openaiCompatible"))
}

/// Emit a deprecation warning when the caller passes provider options under
/// the raw (non-camelCase) key of a hyphenated provider name.
pub fn warn_if_deprecated_provider_options_key(
    raw_name: &str,
    provider_options: Option<&ProviderOptions>,
    warnings: &mut Vec<Warning>,
) {
    let camel = to_camel_case(raw_name);
    if camel == raw_name {
        return;
    }
    if let Some(opts) = provider_options
        && opts.0.contains_key(raw_name)
    {
        warnings.push(Warning::Other {
            message: format!("Use providerOptions key '{camel}' instead of '{raw_name}'."),
        });
    }
}

/// Return the effective options key: camelCase if present, otherwise raw.
pub fn effective_provider_options_key<'a>(
    raw_name: &'a str,
    camel_name: &'a str,
    provider_options: Option<&ProviderOptions>,
) -> &'a str {
    if raw_name != camel_name
        && let Some(opts) = provider_options
        && opts.0.contains_key(camel_name)
    {
        return camel_name;
    }
    raw_name
}

#[cfg(test)]
#[path = "provider_options_key.test.rs"]
mod tests;
