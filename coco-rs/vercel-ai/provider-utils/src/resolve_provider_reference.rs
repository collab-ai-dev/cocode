//! Resolve a `SharedV4ProviderReference` against a target provider.
//!
//! `SharedV4ProviderReference` is `{ provider_name → file_id }`. When
//! a provider receives a tool result with a file-reference part, it
//! calls this helper to look up its own ID.

use vercel_ai_provider::SharedV4ProviderReference;

/// Look up the file ID for `provider` in the reference map.
/// Returns `None` if the map doesn't have an entry for this provider.
pub fn resolve_provider_reference<'a>(
    reference: &'a SharedV4ProviderReference,
    provider: &str,
) -> Option<&'a str> {
    reference.get(provider).map(String::as_str)
}

#[cfg(test)]
#[path = "resolve_provider_reference.test.rs"]
mod tests;
