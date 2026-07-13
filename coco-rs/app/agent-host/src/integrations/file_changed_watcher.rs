//! `FileChanged` hook watcher.
//!
//! Runs a filesystem watcher over paths registered by hook
//! `hookSpecificOutput.watchPaths` from `SessionStart` / `CwdChanged`
//! and fires `executeFileChangedHooks` per file event.
//!
//! Uses `coco_file_watch::FileWatcher` (notify crate), mapped to
//! three event types: `change` / `add` / `unlink`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_file_watch::EventKind;
use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration::FileChangeEvent;
use coco_hooks::orchestration::OrchestrationContext;

/// Throttle window matching the chokidar default debounce.
const FILE_CHANGED_THROTTLE_MS: u64 = 250;

/// One filesystem event mapped to the `FileChanged` shape.
#[derive(Debug, Clone)]
struct FileChangedEvent {
    path: PathBuf,
    kind: FileChangeEvent,
}

/// Owns a `FileWatcher` plus a forwarder task that fires
/// `execute_file_changed` for each detected change.
///
/// Lifecycle is the SessionRuntime's: when the runtime drops, the
/// `FileWatcher` (held in `_watcher`) drops with it, the broadcast
/// channel closes, and the forwarder task exits.
pub struct FileChangedHookWatcher {
    watcher: Arc<FileWatcher<FileChangedEvent>>,
}

impl FileChangedHookWatcher {
    /// Build a watcher that fires `execute_file_changed` against the
    /// supplied registry + context. Returns `None` when watcher
    /// construction fails (e.g. on platforms without `inotify`); the
    /// caller silently degrades to no FileChanged events.
    pub fn new(
        registry: Arc<HookRegistry>,
        ctx_factory: Arc<dyn Fn() -> OrchestrationContext + Send + Sync>,
    ) -> Option<Self> {
        let builder = FileWatcherBuilder::<FileChangedEvent>::new()
            .throttle_interval(Duration::from_millis(FILE_CHANGED_THROTTLE_MS));

        // The classify closure maps notify events into our domain enum.
        // notify's `EventKind` covers Create / Modify / Remove with finer
        // sub-variants; we collapse to the 3-state enum.
        let watcher = builder
            .build(
                |event| {
                    let kind = match event.kind {
                        EventKind::Create(_) => FileChangeEvent::Add,
                        EventKind::Remove(_) => FileChangeEvent::Unlink,
                        EventKind::Modify(_) => FileChangeEvent::Change,
                        // notify also emits Access / Other / Any — these
                        // are not interesting for the FileChanged hook,
                        // so swallow them.
                        _ => return None,
                    };
                    let path = event.paths.first()?.clone();
                    Some(FileChangedEvent { path, kind })
                },
                // Coalescing rule: keep the most recent event per path
                // by replacing. The throttle window in `FileWatcher`
                // already collapses bursts, so `merge` only needs to
                // pick a winner when two events of different kinds
                // collide. Last-write-wins.
                |_old, new| new,
            )
            .ok()?;
        let watcher = Arc::new(watcher);

        // Forwarder task: consume domain events and fire the hook.
        let mut rx = watcher.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                let ctx = (ctx_factory)();
                if ctx.disable_all_hooks {
                    continue;
                }
                let path_str = event.path.display().to_string();
                if let Err(e) = coco_hooks::orchestration::execute_file_changed(
                    &registry, &ctx, &path_str, event.kind,
                )
                .await
                {
                    tracing::warn!(
                        error = %e,
                        path = %path_str,
                        "FileChanged hook firing failed"
                    );
                }
            }
        });

        Some(Self { watcher })
    }

    /// Register one or more absolute paths to watch.
    /// `hookSpecificOutput.watchPaths` from `SessionStart` /
    /// `CwdChanged` aggregates onto the active watcher's path set.
    /// Non-existent paths are silently skipped.
    pub fn add_paths(&self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !path.exists() {
                continue;
            }
            // Files watch as `NonRecursive`; directories use Recursive
            // so all descendants emit.
            let mode = if path.is_dir() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            self.watcher.watch(path, mode);
        }
    }
}
