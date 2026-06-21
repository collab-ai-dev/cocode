//! Root directory watcher for LSP server cleanup.
//!
//! Monitors parent directories of active LSP workspace roots and detects
//! when a root directory is deleted (e.g., git worktree removal). On
//! deletion, emits [`RootDeleted`] events so the server manager can
//! shut down orphaned LSP processes.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use notify::EventKind;
use tokio::sync::broadcast;
use tracing::debug;
use tracing::warn;

/// Throttle interval for coalescing deletion events.
const THROTTLE_INTERVAL: Duration = Duration::from_millis(500);

/// A tracked root directory was deleted from the filesystem.
#[derive(Debug, Clone)]
pub(crate) struct RootDeleted {
    pub roots: Vec<PathBuf>,
}

/// Watches parent directories of active LSP workspace roots.
///
/// When a root directory disappears, the watcher emits a [`RootDeleted`]
/// event via a broadcast channel. The LSP server manager subscribes and
/// shuts down all servers associated with that root.
pub(crate) struct RootWatcher {
    watcher: FileWatcher<RootDeleted>,
    /// parent_dir → set of tracked root paths under it
    parent_to_roots: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>,
}

impl RootWatcher {
    /// Create a new root watcher.
    ///
    /// Returns `None` if the filesystem watcher cannot be initialized
    /// (e.g., unsupported platform or inotify limit reached).
    pub fn new() -> Option<Self> {
        let parent_to_roots: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let map_for_classify = Arc::clone(&parent_to_roots);

        let watcher = FileWatcherBuilder::new()
            .throttle_interval(THROTTLE_INTERVAL)
            .build(
                move |event| classify_event(event, &map_for_classify),
                merge_events,
            )
            .map_err(|e| {
                warn!(error = %e, "Failed to create root directory watcher");
            })
            .ok()?;

        Some(Self {
            watcher,
            parent_to_roots,
        })
    }

    /// Start tracking a root directory for deletion.
    ///
    /// Watches the root's parent directory (non-recursive). Multiple roots
    /// under the same parent share a single OS watch.
    pub fn track_root(&self, root_path: &Path) {
        let Some(parent) = root_path.parent() else {
            return;
        };
        let root = root_path.to_path_buf();
        let parent = parent.to_path_buf();

        let is_new_parent = {
            let mut guard = lock_or_recover(&self.parent_to_roots);
            let roots = guard.entry(parent.clone()).or_default();
            let was_empty = roots.is_empty();
            roots.insert(root.clone());
            was_empty
        };

        if is_new_parent {
            debug!(
                parent = %parent.display(),
                root = %root.display(),
                "Watching parent directory for root deletion"
            );
            self.watcher.watch(parent, RecursiveMode::NonRecursive);
        }
    }

    /// Stop tracking a root directory.
    ///
    /// If no more roots remain under the parent, the OS watch is removed.
    pub fn untrack_root(&self, root_path: &Path) {
        let Some(parent) = root_path.parent() else {
            return;
        };
        let parent = parent.to_path_buf();

        let should_unwatch = {
            let mut guard = lock_or_recover(&self.parent_to_roots);
            if let Some(roots) = guard.get_mut(&parent) {
                roots.remove(root_path);
                if roots.is_empty() {
                    guard.remove(&parent);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_unwatch {
            debug!(
                parent = %parent.display(),
                "Unwatching parent directory (no more tracked roots)"
            );
            self.watcher.unwatch(&parent);
        }
    }

    /// Subscribe to root-deleted events.
    pub fn subscribe(&self) -> broadcast::Receiver<RootDeleted> {
        self.watcher.subscribe()
    }

    /// Get a clone of the parent-to-roots map for the background consumer task.
    pub fn parent_to_roots(&self) -> Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>> {
        Arc::clone(&self.parent_to_roots)
    }
}

/// Classify a filesystem event into a [`RootDeleted`] if it indicates
/// that a tracked root directory was removed.
fn classify_event(
    event: &notify::Event,
    parent_to_roots: &Mutex<HashMap<PathBuf, HashSet<PathBuf>>>,
) -> Option<RootDeleted> {
    // Only consider removal events
    if !matches!(event.kind, EventKind::Remove(_)) {
        return None;
    }

    let guard = lock_or_recover(parent_to_roots);
    let mut deleted_roots = Vec::new();

    for event_path in &event.paths {
        // Look up the parent directory to find tracked roots efficiently (O(1) map lookup)
        if let Some(parent) = event_path.parent()
            && let Some(roots) = guard.get(parent)
            && roots.contains(event_path)
            && !event_path.exists()
        {
            debug!(
                root = %event_path.display(),
                "Detected root directory deletion via filesystem event"
            );
            deleted_roots.push(event_path.clone());
        }
    }

    if deleted_roots.is_empty() {
        None
    } else {
        Some(RootDeleted {
            roots: deleted_roots,
        })
    }
}

/// Merge two [`RootDeleted`] events by combining and deduplicating roots.
fn merge_events(mut acc: RootDeleted, new: RootDeleted) -> RootDeleted {
    for root in new.roots {
        if !acc.roots.contains(&root) {
            acc.roots.push(root);
        }
    }
    acc
}

/// Lock a mutex, recovering from poison if needed.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    }
}

#[cfg(test)]
#[path = "root_watcher.test.rs"]
mod tests;
