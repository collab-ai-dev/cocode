//! Plugin hot-reload -- watches for settings changes and reloads plugin hooks.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Tracks whether the plugin settings have changed since last load.
///
/// Watches for plugin-affecting settings changes by comparing snapshots of
/// enabledPlugins, strictKnownMarketplaces, blockedMarketplaces, and
/// extraKnownMarketplaces.
pub struct PluginReloadTracker {
    /// Whether a reload has been requested.
    needs_reload: Arc<AtomicBool>,
    /// Snapshot of the last known plugin-affecting settings.
    last_snapshot: std::sync::Mutex<Option<String>>,
}

impl PluginReloadTracker {
    pub fn new() -> Self {
        Self {
            needs_reload: Arc::new(AtomicBool::new(false)),
            last_snapshot: std::sync::Mutex::new(None),
        }
    }

    /// Check if a reload is needed and reset the flag.
    pub fn take_reload_needed(&self) -> bool {
        self.needs_reload.swap(false, Ordering::SeqCst)
    }

    /// Mark that a reload is needed.
    pub fn request_reload(&self) {
        self.needs_reload.store(true, Ordering::SeqCst);
    }

    /// Update the settings snapshot and return whether it changed.
    pub fn update_snapshot(&self, new_snapshot: &str) -> bool {
        let mut guard = self
            .last_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = guard.as_deref() != Some(new_snapshot);
        if changed {
            *guard = Some(new_snapshot.to_string());
            self.needs_reload.store(true, Ordering::SeqCst);
        }
        changed
    }

    /// Get a clone of the needs_reload flag for sharing across threads.
    pub fn needs_reload_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.needs_reload)
    }
}

impl Default for PluginReloadTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "hot_reload.test.rs"]
mod tests;
