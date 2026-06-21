//! User keybinding configuration loader with hot-reload support.
//!
//! Loads `~/.coco/keybindings.json`, merges with [`crate::defaults`], runs
//! validation, and emits hot-reload events when the file changes.
//!
//! * Path is `~/.coco/keybindings.json` (or `$COCO_CONFIG_DIR/keybindings.json`)
//!   per the coco-rs config-home rule. Resolution goes through
//!   [`coco_utils_common::find_coco_home`].
//! * User customization is always allowed.
//! * Hot reload uses `coco-file-watch`. The classifier filters to the
//!   target path (we watch the parent dir so create-after-startup
//!   works, but only emit when `keybindings.json` itself changes).
//!
//! Feature-gated behind `loader` so library callers without a Tokio
//! runtime aren't forced to depend on the watcher stack.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::Instrument;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::Keybinding;
use crate::KeybindingsConfig;
use crate::defaults::default_blocks;
use crate::validator::Severity;
use crate::validator::ValidationIssue;
use crate::validator::ValidationKind;
use crate::validator::validate;

/// Result of loading keybindings from disk + defaults.
#[derive(Debug, Clone)]
pub struct KeybindingsLoadResult {
    /// Parsed bindings ready for the resolver. Defaults first, then
    /// user bindings — last-wins via the resolver.
    pub bindings: Vec<Keybinding>,
    /// Validation warnings/errors against the user's portion. Empty
    /// when only defaults are loaded or the user file is clean.
    pub warnings: Vec<ValidationIssue>,
}

/// Loader errors that aren't validation issues.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in {path:?}: {source}")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Default user keybindings path: `<coco_home>/keybindings.json`.
///
/// Honors `$COCO_CONFIG_DIR` per [`coco_utils_common::find_coco_home`].
pub fn default_keybindings_path() -> PathBuf {
    coco_utils_common::find_coco_home().join("keybindings.json")
}

/// Load defaults + the user's `keybindings.json`.
///
/// File missing / unreadable falls back to defaults silently. JSON
/// parse failure or schema-shape errors surface as
/// [`Severity::Error`] warnings while still returning the defaults so
/// the UI keeps functioning.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub async fn load_keybindings(path: &Path) -> KeybindingsLoadResult {
    let default_bindings = default_config_parsed();

    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!("no user keybindings file; using defaults only");
            return KeybindingsLoadResult {
                bindings: default_bindings,
                warnings: vec![],
            };
        }
        Err(e) => {
            warn!("failed to read user keybindings: {e}");
            return KeybindingsLoadResult {
                bindings: default_bindings,
                warnings: vec![ValidationIssue {
                    kind: ValidationKind::ParseError,
                    severity: Severity::Error,
                    message: format!("Could not read {}: {e}", path.display()),
                    context: None,
                    chord: None,
                    suggestion: None,
                }],
            };
        }
    };

    let user_config = match KeybindingsConfig::from_json(&content) {
        Ok(c) => c,
        Err(e) => {
            warn!("invalid JSON in user keybindings: {e}");
            return KeybindingsLoadResult {
                bindings: default_bindings,
                warnings: vec![ValidationIssue {
                    kind: ValidationKind::ParseError,
                    severity: Severity::Error,
                    message: format!("Could not parse {}: {e}", path.display()),
                    context: None,
                    chord: None,
                    suggestion: Some("See https://docs.claude.com/en/keybindings".into()),
                }],
            };
        }
    };

    let warnings = validate(&user_config);
    let mut user_bindings = user_config.parse_bindings();
    let user_count = user_bindings.len();

    let mut all = default_bindings;
    let default_count = all.len();
    all.append(&mut user_bindings);

    info!(
        default_bindings = default_count,
        user_bindings = user_count,
        warnings = warnings.len(),
        "loaded user keybindings",
    );

    KeybindingsLoadResult {
        bindings: all,
        warnings,
    }
}

fn default_config_parsed() -> Vec<Keybinding> {
    let config = KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: default_blocks(),
    };
    config.parse_bindings()
}

/// Hot-reload-capable loader. Watches the user's keybindings file and
/// publishes a fresh [`KeybindingsLoadResult`] on change.
///
/// Drop the [`KeybindingsWatcher`] to stop watching. Subscribers
/// receive a `broadcast::Receiver`; missed messages are dropped (the
/// caller can re-`subscribe` for the latest value via
/// [`KeybindingsWatcher::current`]).
pub struct KeybindingsWatcher {
    path: PathBuf,
    current: Arc<Mutex<KeybindingsLoadResult>>,
    tx: broadcast::Sender<KeybindingsLoadResult>,
    // Held to keep the watcher alive; `_` because we don't read it.
    _watcher: FileWatcher<()>,
}

impl KeybindingsWatcher {
    /// Watch `<coco_home>/keybindings.json`. See [`Self::watch`] for a
    /// custom path.
    pub async fn watch_default() -> Self {
        Self::watch(default_keybindings_path()).await
    }

    /// Build a watcher and load the current state once.
    #[tracing::instrument(skip_all, fields(path = %path.display()))]
    pub async fn watch(path: PathBuf) -> Self {
        let initial = load_keybindings(&path).await;
        let current = Arc::new(Mutex::new(initial));
        let (tx, _) = broadcast::channel(8);

        // The classifier filters notify events to ones that touch the
        // target path. Without this, every file change in the parent
        // directory (sessions, logs, plugin installs, …) triggers a
        // re-read.
        let target = path.clone();
        let watcher = FileWatcherBuilder::<()>::new()
            .throttle_interval(Duration::from_millis(500))
            .build(
                move |event| {
                    if event.paths.iter().any(|p| p == &target) {
                        Some(())
                    } else {
                        None
                    }
                },
                |a, _| a,
            )
            .unwrap_or_else(|err| {
                warn!("keybindings file-watcher init failed: {err}; hot reload disabled");
                FileWatcherBuilder::<()>::new().build_noop()
            });

        // Watch the parent directory (NonRecursive) so create-after-
        // startup is caught. The classify closure above will filter
        // out unrelated changes.
        if let Some(parent) = path.parent()
            && parent.exists()
        {
            watcher.watch(parent.to_path_buf(), RecursiveMode::NonRecursive);
        }

        let mut rx = watcher.subscribe();
        let path_for_loop = path.clone();
        let current_for_loop = current.clone();
        let tx_for_loop = tx.clone();
        tokio::spawn(
            async move {
                while rx.recv().await.is_ok() {
                    let result = load_keybindings(&path_for_loop).await;
                    debug!(
                        warnings = result.warnings.len(),
                        bindings = result.bindings.len(),
                        "hot-reloaded keybindings",
                    );
                    if let Ok(mut guard) = current_for_loop.lock() {
                        *guard = result.clone();
                    }
                    let _ = tx_for_loop.send(result);
                }
            }
            .instrument(info_span!("keybindings_watch_loop")),
        );

        Self {
            path,
            current,
            tx,
            _watcher: watcher,
        }
    }

    /// Path being watched.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Latest load result.
    pub fn current(&self) -> KeybindingsLoadResult {
        match self.current.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Subscribe to hot-reload events. Each `recv()` yields the new
    /// load result (or a `RecvError::Lagged` if the subscriber
    /// fell behind — caller should call [`Self::current`] in that
    /// case).
    pub fn subscribe(&self) -> broadcast::Receiver<KeybindingsLoadResult> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
#[path = "loader.test.rs"]
mod tests;
