//! Plugin change detection.
//!
//! Watches plugin settings on disk and shows a "Plugins changed. Run
//! /reload-plugins to activate." notification — never auto-reloads. Uses a
//! debounced file watcher across the plugin scopes (user / project / managed)
//! plus the `installed_plugins.json` ledger.
//!
//! The watcher is intentionally *not* hooked into the plugin refresh path
//! ([`crate::load_enabled_plugins`]) — that's the explicit `/reload-plugins`
//! user action. This module only surfaces the *fact* that something changed.
//!
//! ## Reuse note
//!
//! This module is a *thin* wrapper around [`coco_file_watch::FileWatcher`]:
//! it supplies the classify/merge closures and the plugin-scope path list.
//! Subscription, throttling, the OS-event pump, and per-path bookkeeping
//! all live in `coco-file-watch`. The wrapper exists solely so callers
//! can ask for "the plugin watcher" without re-stating the policy.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use tokio::sync::broadcast;

/// Debounce interval for plugin file changes.
const PLUGIN_DEBOUNCE_MS: u64 = 300;

/// One plugin-scope file mutation, coalesced over [`PLUGIN_DEBOUNCE_MS`].
///
/// `reason` is a short human-readable description suitable for a
/// status-line notification (e.g. `"installed_plugins.json changed"`,
/// `"PLUGIN.toml added"`). The exact paths that changed are included so
/// consumers that care (telemetry, transcripts) can log them.
#[derive(Debug, Clone, Default)]
pub struct PluginsChanged {
    /// Paths that triggered the event.
    pub changed_paths: Vec<PathBuf>,
    /// Short reason string suitable for the user-facing banner.
    pub reason: String,
}

/// Watcher over plugin scopes (user / project / managed / installed
/// ledger). Emits a [`PluginsChanged`] on every debounced burst; never
/// auto-reloads.
///
/// Subscribers (e.g. CLI bootstrap) read events via [`Self::subscribe`]
/// and bridge them into `CoreEvent::Protocol(ServerNotification::PluginsChanged)`.
///
/// ## Lifecycle
///
/// The watcher's background tasks live inside the wrapped
/// [`FileWatcher`]; when the last `Arc<PluginChangeDetector>` is
/// dropped, the watcher (and its notify thread) drop with it. Callers
/// hold the `Arc` in a guard binding for the session lifetime.
pub struct PluginChangeDetector {
    /// Wrapped generic watcher — owns the OS-event pump, throttle
    /// timer, and broadcast channel. `subscribe()` passes through.
    inner: FileWatcher<PluginsChanged>,
    /// The paths the caller asked us to observe — preserved verbatim
    /// (including any that did not exist when [`Self::new`] ran, since
    /// [`FileWatcher::try_watch`] silently no-ops on missing paths).
    requested_paths: Vec<PathBuf>,
}

impl PluginChangeDetector {
    /// Build a watcher and register every path in `paths`. Directory
    /// paths are watched recursively; file paths non-recursively.
    /// Missing paths are silently skipped (deferred to a future create
    /// event under the parent directory).
    pub fn new(paths: Vec<PathBuf>) -> crate::Result<Arc<Self>> {
        let inner = FileWatcherBuilder::new()
            .throttle_interval(Duration::from_millis(PLUGIN_DEBOUNCE_MS))
            .build(classify, merge)
            .map_err(|e| {
                crate::PluginError::generic(
                    "plugin-watcher",
                    format!("file-watch build failed: {e}"),
                )
            })?;

        for path in &paths {
            // `try_watch` already returns Ok(()) for non-existent paths
            // and logs failures internally — no pre-check needed.
            let mode = if path.is_dir() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            inner.watch(path.clone(), mode);
        }

        Ok(Arc::new(Self {
            inner,
            requested_paths: paths,
        }))
    }

    /// Subscribe to plugin-change notifications. Pass-through to the
    /// wrapped [`FileWatcher::subscribe`].
    pub fn subscribe(&self) -> broadcast::Receiver<PluginsChanged> {
        self.inner.subscribe()
    }

    /// Paths the caller asked the watcher to observe. Returned in the
    /// order they were passed to [`Self::new`]; the slice may include
    /// paths that did not exist at construction time.
    pub fn watched_paths(&self) -> &[PathBuf] {
        &self.requested_paths
    }
}

// ─── classify + merge closures (extracted so they're testable) ──────────

fn classify(event: &coco_file_watch::Event) -> Option<PluginsChanged> {
    let interesting: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|p| is_interesting_plugin_path(p))
        .cloned()
        .collect();
    if interesting.is_empty() {
        return None;
    }
    Some(PluginsChanged {
        reason: derive_reason(&interesting),
        changed_paths: interesting,
    })
}

fn merge(mut acc: PluginsChanged, new: PluginsChanged) -> PluginsChanged {
    acc.changed_paths.extend(new.changed_paths);
    if acc.reason.is_empty() {
        acc.reason = new.reason;
    }
    acc
}

/// Heuristic — accept paths that look like plugin scopes.
///
/// The notify crate already scopes to the directories we registered,
/// so a coarse accept-all is safe; the throttle is the real
/// backpressure. We reject only the obvious editor-temp / swap noise.
fn is_interesting_plugin_path(path: &std::path::Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.is_empty() {
        return false;
    }
    if name.ends_with('~') || name.starts_with(".#") || name.ends_with(".swp") {
        return false;
    }
    true
}

fn derive_reason(paths: &[PathBuf]) -> String {
    // Pick the most informative file we can name. Pre-compose the reason
    // so the UI doesn't need to peek into the path list.
    let first = paths
        .iter()
        .find_map(|p| p.file_name().and_then(|n| n.to_str()))
        .unwrap_or("plugin state");
    match first {
        "installed_plugins.json" => "installed_plugins.json changed".to_string(),
        "PLUGIN.toml" => "PLUGIN.toml changed".to_string(),
        "marketplace.json" => "marketplace.json changed".to_string(),
        other => format!("{other} changed"),
    }
}

#[cfg(test)]
#[path = "watcher.test.rs"]
mod tests;
