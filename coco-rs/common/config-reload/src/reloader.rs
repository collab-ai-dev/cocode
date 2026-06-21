//! Spawned hot-reload loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RuntimeConfig;
use coco_config::RuntimeOverrides;
use coco_config::RuntimePublisher;
use coco_config::SettingsWatcher;
use coco_config::WatchedKind;
use coco_config::build_runtime_config_with;
use coco_config::load_settings_with;
use coco_file_watch::Event as FsEvent;
use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;

const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Classification of a tracked path. Used for log enrichment (so
/// telemetry can distinguish "policy override changed" from
/// "developer edited project settings") and to keep `WatchedKind`'s
/// public surface useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedKind {
    Settings(WatchedKind),
    FlagSettings,
}

impl TrackedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settings(WatchedKind::Settings(s)) => s.as_str(),
            Self::Settings(WatchedKind::ProvidersCatalog) => "providers_catalog",
            Self::Settings(WatchedKind::ModelsCatalog) => "models_catalog",
            Self::FlagSettings => "flag_settings",
        }
    }
}

/// Domain event emitted on a tracked-config-path change.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    pub path: PathBuf,
    pub kind: TrackedKind,
}

/// Failed rebuild for a tracked config-path change. Subscribers use
/// this to surface reload failures while the reloader keeps the prior
/// valid runtime snapshot.
#[derive(Debug, Clone)]
pub struct ConfigReloadError {
    pub path: PathBuf,
    pub kind: TrackedKind,
    pub message: String,
}

/// Builder for [`RuntimeReloader::spawn`].
pub struct ReloadOptions {
    cwd: PathBuf,
    flag_settings: Option<PathBuf>,
    overrides: RuntimeOverrides,
    catalogs: CatalogPaths,
    env_factory: Box<dyn FnMut() -> EnvSnapshot + Send + 'static>,
    debounce: Duration,
    /// Raw `--setting-sources` CSV. `None` ⇒ all sources enabled. Resolved
    /// once at spawn and reused for every hot-reload rebuild so reloads
    /// honor the same source filter the initial build used.
    setting_sources: Option<String>,
}

impl ReloadOptions {
    /// Construct with sensible production defaults: live-env snapshot
    /// per rebuild, default catalog paths, no flag settings,
    /// `RuntimeOverrides::default`.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            flag_settings: None,
            overrides: RuntimeOverrides::default(),
            catalogs: CatalogPaths::default(),
            env_factory: Box::new(EnvSnapshot::from_current_process),
            debounce: DEFAULT_DEBOUNCE,
            setting_sources: None,
        }
    }

    pub fn with_flag_settings(mut self, path: impl Into<PathBuf>) -> Self {
        self.flag_settings = Some(path.into());
        self
    }

    /// Restrict which setting sources participate (`--setting-sources` CSV).
    /// `None` ⇒ all sources.
    pub fn with_setting_sources(mut self, csv: Option<String>) -> Self {
        self.setting_sources = csv;
        self
    }

    pub fn with_overrides(mut self, overrides: RuntimeOverrides) -> Self {
        self.overrides = overrides;
        self
    }

    pub fn with_catalog_paths(mut self, catalogs: CatalogPaths) -> Self {
        self.catalogs = catalogs;
        self
    }

    /// Override the env-snapshot factory. Tests pass a fixed snapshot
    /// for determinism; production uses the default
    /// `EnvSnapshot::from_current_process`.
    pub fn with_env_factory<F>(mut self, factory: F) -> Self
    where
        F: FnMut() -> EnvSnapshot + Send + 'static,
    {
        self.env_factory = Box::new(factory);
        self
    }

    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }
}

/// Spawned reloader. Drop aborts the task; callers don't need to opt
/// in via a separate `stop()` call.
pub struct RuntimeReloader {
    publisher: Arc<RuntimePublisher>,
    error_tx: tokio::sync::broadcast::Sender<ConfigReloadError>,
    handle: JoinHandle<()>,
    /// Owns the `FileWatcher`. Drop releases OS-level watcher handles
    /// and closes the broadcast `Sender`; the field is load-bearing
    /// for cleanup so it must not be renamed to start with an
    /// underscore.
    watcher: FileWatcher<ConfigChange>,
}

impl RuntimeReloader {
    /// Build the runtime once, install watchers on every tracked
    /// path's parent directory, and spawn a tokio task that publishes
    /// a fresh `Arc<RuntimeConfig>` to subscribers on every change.
    ///
    /// **Precondition:** must be called from within a Tokio runtime
    /// (uses `tokio::spawn`). Returns `Err` otherwise rather than
    /// panicking.
    ///
    /// **Race window.** Filesystem changes between the initial build
    /// and watch installation are missed; the next save catches up.
    /// This is intrinsic to file watching and not specific to this
    /// reloader.
    ///
    /// **Catalog file appearance.** The watcher subscribes to each
    /// path's *parent directory* non-recursively and filters events
    /// by exact path in `classify`, so a first-time `touch` of
    /// `~/.coco/providers.json` triggers a rebuild even though the
    /// file did not exist at watcher-install time.
    pub fn spawn(opts: ReloadOptions) -> Result<Self, coco_error::BoxedError> {
        Handle::try_current().map_err(|_| {
            coco_error::boxed_err(coco_error::PlainError::new(
                "RuntimeReloader::spawn must be called from a Tokio runtime",
                coco_error::StatusCode::Internal,
            ))
        })?;

        let ReloadOptions {
            cwd,
            flag_settings,
            overrides,
            catalogs,
            mut env_factory,
            debounce,
            setting_sources,
        } = opts;

        // Resolve the `--setting-sources` filter once; every rebuild reuses it.
        let enabled = coco_config::parse_enabled_setting_sources(setting_sources.as_deref());

        // Install the watcher FIRST so any filesystem change between
        // the initial build and watch-install is captured by the
        // broadcast channel and replayed by the spawned task.
        let settings_watcher = SettingsWatcher::with_catalogs(&cwd, &catalogs);
        let mut watch_set: Vec<(PathBuf, RecursiveMode)> = Vec::new();
        let mut tracked_files: Vec<PathBuf> = Vec::new();
        let mut tracked_kinds: Vec<TrackedKind> = Vec::new();

        for (kind, path) in settings_watcher.watched_paths() {
            tracked_files.push(path.clone());
            tracked_kinds.push(TrackedKind::Settings(*kind));
            if let Some(parent) = path.parent() {
                watch_set.push((parent.to_path_buf(), RecursiveMode::NonRecursive));
            }
        }

        // Flag-settings file (CLI `--settings <path>`). Watching its
        // parent dir lets the user edit the file in place and pick up
        // the change without restart.
        if let Some(flag_path) = &flag_settings {
            tracked_files.push(flag_path.clone());
            tracked_kinds.push(TrackedKind::FlagSettings);
            if let Some(parent) = flag_path.parent() {
                watch_set.push((parent.to_path_buf(), RecursiveMode::NonRecursive));
            }
        }

        // Build per-tracked-file matching keys. Some platforms (notably
        // macOS FSEvents) report event paths via canonicalized parents
        // (`/private/var/...`) while the configured paths point at the
        // original symlink (`/var/...`). Match on both the original and
        // a canonicalized-parent variant so the classify closure sees
        // the event regardless of which form notify surfaces.
        let tracked_keys: Vec<(PathBuf, PathBuf, TrackedKind)> = tracked_files
            .iter()
            .zip(tracked_kinds.iter().copied())
            .map(|(path, kind)| {
                let canonical = path
                    .parent()
                    .and_then(|parent| std::fs::canonicalize(parent).ok())
                    .and_then(|canon| path.file_name().map(|name| canon.join(name)))
                    .unwrap_or_else(|| path.clone());
                (path.clone(), canonical, kind)
            })
            .collect();
        let watcher = FileWatcherBuilder::<ConfigChange>::new()
            .throttle_interval(debounce)
            .build(
                move |ev: &FsEvent| {
                    ev.paths.iter().find_map(|p| {
                        tracked_keys
                            .iter()
                            .find(|(path, canonical, _)| path == p || canonical == p)
                            .map(|(path, _, kind)| ConfigChange {
                                path: path.clone(),
                                kind: *kind,
                            })
                    })
                },
                |_old, new| new,
            )
            .map_err(|e| coco_error::boxed(e, coco_error::StatusCode::IoError))?;

        let install_failures = install_watches(&watcher, &watch_set);
        if install_failures > 0 {
            warn!(
                failures = install_failures,
                total = watch_set.len(),
                "some watch installs failed; hot-reload coverage degraded"
            );
        }

        // Initial build AFTER watcher install. Any save during this
        // window is buffered in the broadcast channel and surfaced by
        // the spawned task's first `rx.recv().await`.
        let initial = build_with(
            &cwd,
            flag_settings.as_deref(),
            &env_factory(),
            &overrides,
            &catalogs,
            &enabled,
        )?;
        let publisher = Arc::new(RuntimePublisher::new(Arc::new(initial)));
        let (error_tx, _) = tokio::sync::broadcast::channel(32);

        let publisher_for_task = publisher.clone();
        let error_tx_for_task = error_tx.clone();
        let mut rx = watcher.subscribe();
        let handle = tokio::spawn(async move {
            while let Ok(change) = rx.recv().await {
                trace!(
                    path = %change.path.display(),
                    kind = change.kind.as_str(),
                    "config-watch event"
                );
                match build_with(
                    &cwd,
                    flag_settings.as_deref(),
                    &env_factory(),
                    &overrides,
                    &catalogs,
                    &enabled,
                ) {
                    Ok(runtime) => {
                        info!(
                            path = %change.path.display(),
                            kind = change.kind.as_str(),
                            "config change → rebuilt RuntimeConfig"
                        );
                        publisher_for_task.publish(Arc::new(runtime));
                    }
                    Err(err) => {
                        let message = err.to_string();
                        let _ = error_tx_for_task.send(ConfigReloadError {
                            path: change.path.clone(),
                            kind: change.kind,
                            message: message.clone(),
                        });
                        error!(
                            path = %change.path.display(),
                            kind = change.kind.as_str(),
                            error = %message,
                            "config rebuild failed; keeping prior snapshot"
                        );
                    }
                }
            }
            warn!("file-watch broadcast closed; reload loop exiting");
        });

        Ok(Self {
            publisher,
            error_tx,
            handle,
            watcher,
        })
    }

    /// Borrow the publisher. Subscribers call
    /// `publisher().subscribe()` to obtain a `watch::Receiver` and
    /// `publisher().current()` for the latest snapshot.
    pub fn publisher(&self) -> Arc<RuntimePublisher> {
        self.publisher.clone()
    }

    /// Subscribe to raw [`ConfigChange`] events — one item per detected
    /// settings/catalog file change, keyed by the change source. Use
    /// this to fire the `ConfigChange` hook (TS
    /// `executeConfigChangeHooks(source, path)`) without re-running
    /// the reload pipeline. Each subscriber gets an independent
    /// `broadcast::Receiver`; lagging subscribers see a `RecvError::Lagged`
    /// rather than blocking the reload loop.
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<ConfigChange> {
        self.watcher.subscribe()
    }

    /// Subscribe to failed reload attempts. Successful reloads are
    /// published through [`Self::publisher`]; failures never replace
    /// the prior snapshot, so this side-channel is the UI-visible
    /// diagnostic path.
    pub fn subscribe_errors(&self) -> tokio::sync::broadcast::Receiver<ConfigReloadError> {
        self.error_tx.subscribe()
    }

    /// Read the latest snapshot.
    pub fn current(&self) -> Arc<RuntimeConfig> {
        self.publisher.current()
    }

    /// **Test helper** — steal the spawned task's `JoinHandle` so a
    /// test can `await` its termination after `Drop` runs `.abort()`.
    /// Replaces the field with a no-op handle so `Drop` remains
    /// idempotent. Not exposed outside `cfg(test)` builds.
    #[cfg(test)]
    pub fn steal_join_handle_for_test(&mut self) -> JoinHandle<()> {
        // Replace with a handle that is already aborted-and-done so
        // Drop's `self.handle.abort()` is a no-op.
        let placeholder = tokio::spawn(async {});
        placeholder.abort();
        std::mem::replace(&mut self.handle, placeholder)
    }
}

impl Drop for RuntimeReloader {
    fn drop(&mut self) {
        // Explicit task termination. The broadcast Sender held by
        // `watcher` would also close on field-drop, but aborting the
        // JoinHandle guarantees a clean stop independent of field
        // ordering.
        self.handle.abort();
        // `watcher` field then drops on its own, releasing the OS
        // watcher and broadcast Sender.
        let _ = &self.watcher;
    }
}

fn build_with(
    cwd: &std::path::Path,
    flag_settings: Option<&std::path::Path>,
    env: &EnvSnapshot,
    overrides: &RuntimeOverrides,
    catalogs: &CatalogPaths,
    enabled: &std::collections::HashSet<coco_config::SettingSource>,
) -> Result<RuntimeConfig, coco_error::BoxedError> {
    let to_boxed = |e| {
        coco_error::boxed_err(coco_error::PlainError::new(
            format!("{e}"),
            coco_error::StatusCode::InvalidConfig,
        ))
    };
    let settings = load_settings_with(
        cwd,
        flag_settings,
        &catalogs.user_settings,
        &catalogs.managed_settings,
        enabled,
    )
    .map_err(to_boxed)?;
    build_runtime_config_with(
        settings,
        env.clone(),
        overrides.clone(),
        catalogs.clone(),
        enabled.clone(),
    )
    .map_err(to_boxed)
}

/// Install every watch in `watch_set`, returning the count of
/// failures. Each failure is logged at `warn`. Caller may use the
/// count for an aggregate diagnostic.
fn install_watches(
    watcher: &FileWatcher<ConfigChange>,
    watch_set: &[(PathBuf, RecursiveMode)],
) -> usize {
    let mut failures = 0;
    for (path, mode) in watch_set {
        if let Err(err) = watcher.try_watch(path.clone(), *mode) {
            warn!(
                path = %path.display(),
                error = %err,
                "watch install failed"
            );
            failures += 1;
        }
    }
    failures
}

#[cfg(test)]
#[path = "reloader.test.rs"]
mod tests;
