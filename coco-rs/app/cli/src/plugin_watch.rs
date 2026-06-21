//! Spawn the plugin-change watcher and bridge events to a session's
//! `notification_tx`.
//!
//! Single source of truth for the wire-up that both interactive (TUI)
//! and SDK paths need. Plugin changes surface as a notification
//! ("Plugins changed. Run /reload-plugins to activate."), never an
//! auto-reload.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_plugins::watcher::PluginChangeDetector;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use tokio::sync::mpsc;

/// Default plugin scopes watched in every session:
/// - `<cwd>/.coco/plugins` — project scope
/// - `<config_home>/plugins` — user scope (recursive)
/// - `<config_home>/plugins/installed_plugins.json` — ledger file
///   (already covered by the recursive watch above but listed for
///   documentation; [`PluginChangeDetector::new`] dedupes via
///   [`coco_file_watch::FileWatcher::try_watch`]).
pub fn default_watch_paths(cwd: &Path, config_home: &Path) -> Vec<PathBuf> {
    vec![
        cwd.join(".coco").join("plugins"),
        config_home.join("plugins"),
    ]
}

/// Spawn the watcher + the forwarder task that lifts every debounced
/// burst into a `ServerNotification::PluginsChanged` on `notify_tx`.
///
/// Returns the `Arc<PluginChangeDetector>` the caller must hold for the
/// session lifetime (drop = clean shutdown). Returns `None` when
/// construction fails (logged at `warn`); the session continues
/// without the banner rather than aborting.
pub fn spawn(
    notify_tx: mpsc::Sender<CoreEvent>,
    cwd: &Path,
    config_home: &Path,
) -> Option<Arc<PluginChangeDetector>> {
    let paths = default_watch_paths(cwd, config_home);
    match PluginChangeDetector::new(paths) {
        Ok(detector) => {
            let mut rx = detector.subscribe();
            tokio::spawn(async move {
                while let Ok(change) = rx.recv().await {
                    let _ = notify_tx
                        .send(CoreEvent::Protocol(ServerNotification::PluginsChanged {
                            reason: change.reason,
                        }))
                        .await;
                }
            });
            Some(detector)
        }
        Err(err) => {
            tracing::warn!("plugin watcher disabled: {err}");
            None
        }
    }
}
