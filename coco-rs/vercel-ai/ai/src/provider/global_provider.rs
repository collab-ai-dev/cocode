//! Global default provider pattern.
//!
//! This module provides a global default provider that can be set once
//! and used for all model resolution via string model IDs.

// Allow expect on RwLock since poisoned locks are unrecoverable
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::RwLock;

use once_cell::sync::Lazy;
use vercel_ai_provider::ProviderV4;

/// Global default provider storage.
static DEFAULT_PROVIDER: Lazy<RwLock<Option<Arc<dyn ProviderV4>>>> =
    Lazy::new(|| RwLock::new(None));

/// Set the global default provider.
///
/// This should be called once at application startup to configure
/// the default provider for model resolution.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::set_default_provider;
/// use std::sync::Arc;
///
/// set_default_provider(Arc::new(my_provider));
/// ```
pub fn set_default_provider(provider: Arc<dyn ProviderV4>) {
    let mut guard = DEFAULT_PROVIDER
        .write()
        .expect("global provider lock poisoned");
    *guard = Some(provider);
}

/// Get the global default provider.
///
/// Returns `None` if no default provider has been set.
pub fn get_default_provider() -> Option<Arc<dyn ProviderV4>> {
    let guard = DEFAULT_PROVIDER
        .read()
        .expect("global provider lock poisoned");
    guard.clone()
}

/// Clear the global default provider.
///
/// This is primarily useful for testing.
pub fn clear_default_provider() {
    let mut guard = DEFAULT_PROVIDER
        .write()
        .expect("global provider lock poisoned");
    *guard = None;
}

/// Check if a default provider is set.
pub fn has_default_provider() -> bool {
    let guard = DEFAULT_PROVIDER
        .read()
        .expect("global provider lock poisoned");
    guard.is_some()
}

#[cfg(test)]
#[path = "global_provider.test.rs"]
mod tests;
