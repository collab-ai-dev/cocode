//! In-memory store for env vars set via `/env`.
//!
//! Entries are applied as env-overrides of every child shell spawn (but NOT
//! to the coco process itself). An `Arc<RwLock<HashMap<...>>>`-backed
//! `SessionEnvVars` that callers clone and pass into the shell provider.
//! One instance per session.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

/// Session-scoped env vars applied to spawned shells.
#[derive(Debug, Default, Clone)]
pub struct SessionEnvVars {
    inner: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionEnvVars {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace `name = value`.
    pub fn set(&self, name: impl Into<String>, value: impl Into<String>) {
        if let Ok(mut g) = self.inner.write() {
            g.insert(name.into(), value.into());
        }
    }

    /// Remove `name`. Returns the prior value if any.
    pub fn delete(&self, name: &str) -> Option<String> {
        self.inner.write().ok().and_then(|mut g| g.remove(name))
    }

    /// Drop every entry.
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.write() {
            g.clear();
        }
    }

    /// Snapshot the current entries.
    pub fn snapshot(&self) -> HashMap<String, String> {
        self.inner.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// True when no entries are set.
    pub fn is_empty(&self) -> bool {
        self.inner.read().map(|g| g.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
#[path = "vars.test.rs"]
mod tests;
