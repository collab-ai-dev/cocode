//! Generic, reusable file-watch infrastructure.
//!
//! Provides [`FileWatcher`] — a throttled, event-coalescing file watcher that
//! bridges OS filesystem events into typed domain events via caller-supplied
//! `classify` and `merge` closures.
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use coco_file_watch::{FileWatcherBuilder, RecursiveMode};
//!
//! #[derive(Debug, Clone)]
//! struct ConfigChanged(Vec<PathBuf>);
//!
//! let watcher = FileWatcherBuilder::new()
//!     .throttle_interval(Duration::from_millis(500))
//!     .build(
//!         |event| {
//!             let paths: Vec<PathBuf> = event.paths.clone();
//!             if paths.is_empty() { None } else { Some(ConfigChanged(paths)) }
//!         },
//!         |mut acc, new| { acc.0.extend(new.0); acc },
//!     )
//!     .unwrap();
//!
//! let mut rx = watcher.subscribe();
//! watcher.watch("/tmp/config".into(), RecursiveMode::Recursive);
//! ```

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::sleep_until;
use tracing::trace;
use tracing::warn;

pub use notify::Event;
pub use notify::EventKind;
pub use notify::RecursiveMode;
use notify::Watcher;

const DEFAULT_THROTTLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_CHANNEL_CAPACITY: usize = 128;

// ---------------------------------------------------------------------------
// ThrottledPaths
// ---------------------------------------------------------------------------

/// Coalesces burst filesystem events, emitting at most once per interval.
pub struct ThrottledPaths {
    pending: HashSet<PathBuf>,
    next_allowed_at: Instant,
    interval: Duration,
}

impl ThrottledPaths {
    pub fn new(interval: Duration) -> Self {
        Self {
            pending: HashSet::new(),
            next_allowed_at: Instant::now(),
            interval,
        }
    }

    /// Add paths to the pending set.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.pending.extend(paths);
    }

    /// Return accumulated paths if the throttle window has elapsed.
    pub fn take_ready(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        if self.pending.is_empty() || now < self.next_allowed_at {
            return None;
        }
        Some(self.drain(now))
    }

    /// Return accumulated paths regardless of the throttle window (e.g. on shutdown).
    pub fn take_pending(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.drain(now))
    }

    /// Deadline at which the next emission is allowed, or `None` if there is
    /// nothing pending or the window has already elapsed.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        (!self.pending.is_empty() && now < self.next_allowed_at).then_some(self.next_allowed_at)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn drain(&mut self, now: Instant) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.pending.drain().collect();
        paths.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
        self.next_allowed_at = now + self.interval;
        paths
    }
}

// ---------------------------------------------------------------------------
// WatcherInner
// ---------------------------------------------------------------------------

struct WatcherInner {
    watcher: notify::RecommendedWatcher,
    watched_paths: HashMap<PathBuf, RecursiveMode>,
}

// ---------------------------------------------------------------------------
// FileWatcher<E>
// ---------------------------------------------------------------------------

/// Generic file watcher: bridges OS events into typed, throttled domain events.
///
/// `E` is the domain event type produced by the caller-supplied `classify`
/// closure and coalesced via `merge`.
pub struct FileWatcher<E> {
    inner: Option<Mutex<WatcherInner>>,
    tx: broadcast::Sender<E>,
}

impl<E: Clone + Send + 'static> FileWatcher<E> {
    /// Subscribe to domain events emitted by this watcher.
    pub fn subscribe(&self) -> broadcast::Receiver<E> {
        self.tx.subscribe()
    }

    /// Start watching `path` for filesystem changes. Errors are
    /// logged at `warn` and otherwise swallowed; use [`Self::try_watch`]
    /// for a `Result`-returning variant.
    pub fn watch(&self, path: PathBuf, mode: RecursiveMode) {
        if let Err(err) = self.try_watch(path.clone(), mode) {
            warn!("watch install failed for {}: {err}", path.display());
        }
    }

    /// Start watching `path` and return a typed error on failure.
    /// Returns `Ok(())` even if the path doesn't exist (the caller
    /// may be subscribing to a future-create event via the parent
    /// directory).
    pub fn try_watch(&self, path: PathBuf, mode: RecursiveMode) -> notify::Result<()> {
        let Some(inner) = &self.inner else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let mut guard = match inner.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if let Some(existing) = guard.watched_paths.get(&path) {
            if *existing == RecursiveMode::Recursive || *existing == mode {
                return Ok(());
            }
            // Upgrading from NonRecursive → Recursive: unwatch first.
            if let Err(err) = guard.watcher.unwatch(&path) {
                warn!("failed to unwatch {}: {err}", path.display());
            }
        }
        guard.watcher.watch(&path, mode)?;
        guard.watched_paths.insert(path, mode);
        Ok(())
    }

    /// Stop watching `path`.
    pub fn unwatch(&self, path: &Path) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut guard = match inner.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if guard.watched_paths.remove(path).is_some()
            && let Err(err) = guard.watcher.unwatch(path)
        {
            warn!("failed to unwatch {}: {err}", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// FileWatcherBuilder<E>
// ---------------------------------------------------------------------------

/// Builder for [`FileWatcher`].
pub struct FileWatcherBuilder<E> {
    throttle_interval: Duration,
    channel_capacity: usize,
    _marker: std::marker::PhantomData<E>,
}

impl<E: Clone + Send + 'static> Default for FileWatcherBuilder<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Clone + Send + 'static> FileWatcherBuilder<E> {
    pub fn new() -> Self {
        Self {
            throttle_interval: DEFAULT_THROTTLE_INTERVAL,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            _marker: std::marker::PhantomData,
        }
    }

    /// Set the throttle interval (default: 1 second).
    pub fn throttle_interval(mut self, interval: Duration) -> Self {
        self.throttle_interval = interval;
        self
    }

    /// Set the broadcast channel capacity (default: 128).
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Build a live watcher.
    ///
    /// - `classify`: maps a raw [`notify::Event`] to an optional domain event.
    /// - `merge`: combines two domain events accumulated during a throttle window.
    pub fn build<C, M>(self, classify: C, merge: M) -> notify::Result<FileWatcher<E>>
    where
        C: Fn(&notify::Event) -> Option<E> + Send + 'static,
        M: Fn(E, E) -> E + Send + 'static,
    {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let raw_tx_clone = raw_tx;
        let watcher = notify::recommended_watcher(move |res| {
            let _ = raw_tx_clone.send(res);
        })?;
        let (tx, _) = broadcast::channel(self.channel_capacity);
        let file_watcher = FileWatcher {
            inner: Some(Mutex::new(WatcherInner {
                watcher,
                watched_paths: HashMap::new(),
            })),
            tx: tx.clone(),
        };
        spawn_event_loop(raw_rx, tx, self.throttle_interval, classify, merge);
        Ok(file_watcher)
    }

    /// Build a no-op watcher (for tests). Subscribe returns a receiver that
    /// never fires; `watch`/`unwatch` are safe no-ops.
    pub fn build_noop(self) -> FileWatcher<E> {
        let (tx, _) = broadcast::channel(1);
        FileWatcher { inner: None, tx }
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Whether a raw filesystem event represents an actual change to a file's
/// content, name, or existence — as opposed to a read-open/close or a
/// metadata-only touch (atime, permissions).
///
/// Read events (`Access`) and metadata-only events (`Modify(Metadata)`) are
/// dropped *before* the caller's `classify` closure runs. This is
/// load-bearing: notify's inotify backend watches with a mask that includes
/// `IN_OPEN` / `IN_CLOSE_WRITE` / `IN_ATTRIB`, so any consumer whose reaction
/// re-reads the watched file would otherwise self-feed — the reaction's own
/// `open()` re-fires the watch (and under a `strictatime` mount, so does the
/// atime bump), producing an unbounded reload loop. `Any` / `Other` are
/// conservatively treated as changes so backends that only emit coarse events
/// (some non-inotify platforms) are not silently dropped.
fn is_content_change(kind: EventKind) -> bool {
    use notify::event::ModifyKind;
    !matches!(
        kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
    )
}

fn spawn_event_loop<E, C, M>(
    mut raw_rx: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    tx: broadcast::Sender<E>,
    throttle_interval: Duration,
    classify: C,
    merge: M,
) where
    E: Clone + Send + 'static,
    C: Fn(&notify::Event) -> Option<E> + Send + 'static,
    M: Fn(E, E) -> E + Send + 'static,
{
    let Ok(handle) = Handle::try_current() else {
        warn!("file watcher loop skipped: no Tokio runtime available");
        return;
    };
    handle.spawn(async move {
        let mut pending: Option<E> = None;
        let mut next_allowed_at = Instant::now();

        loop {
            let now = Instant::now();
            let has_pending = pending.is_some();
            let timer_deadline = if has_pending && now < next_allowed_at {
                next_allowed_at
            } else {
                // Far future — only wake on channel activity.
                now + Duration::from_secs(60 * 60 * 24 * 365)
            };
            let timer = sleep_until(timer_deadline);
            tokio::pin!(timer);

            tokio::select! {
                res = raw_rx.recv() => {
                    match res {
                        Some(Ok(event)) => {
                            trace!(
                                event_kind = ?event.kind,
                                event_paths = ?event.paths,
                                "file watcher received filesystem event"
                            );
                            // Drop read-opens/closes and metadata-only touches
                            // before classify so a consumer that re-reads the
                            // watched file cannot self-feed a reload loop.
                            if !is_content_change(event.kind) {
                                continue;
                            }
                            if let Some(classified) = classify(&event) {
                                pending = Some(match pending.take() {
                                    Some(acc) => merge(acc, classified),
                                    None => classified,
                                });
                            }
                            let now = Instant::now();
                            if now >= next_allowed_at
                                && let Some(e) = pending.take() {
                                    let _ = tx.send(e);
                                    next_allowed_at = now + throttle_interval;
                                }
                        }
                        Some(Err(err)) => {
                            warn!("file watcher error: {err}");
                        }
                        None => {
                            // Channel closed — flush any pending event.
                            if let Some(e) = pending.take() {
                                let _ = tx.send(e);
                            }
                            break;
                        }
                    }
                }
                _ = &mut timer => {
                    let now = Instant::now();
                    if now >= next_allowed_at
                        && let Some(e) = pending.take() {
                            let _ = tx.send(e);
                            next_allowed_at = now + throttle_interval;
                        }
                }
            }
        }
    });
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
