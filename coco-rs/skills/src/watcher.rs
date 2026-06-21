//! Skill change detection and dynamic reload.
//!
//! Watches skill directories for `.md` file changes, debounces, and triggers
//! skill reload.
//!
//! ## Reuse note
//!
//! Thin wrapper around [`coco_file_watch::FileWatcher`] (mirror
//! [`coco_plugins::watcher::PluginChangeDetector`]). Caller holds the
//! returned `Arc<SkillChangeDetector>` as a guard binding for the
//! session lifetime; dropping the Arc shuts the watcher down cleanly.

use coco_file_watch::FileWatcher;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::SkillDefinition;
use crate::SkillManager;
use crate::SkillScopes;

/// Disk scopes to rebuild during hot reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillReloadScope {
    Managed(PathBuf),
    User(PathBuf),
    Project(PathBuf),
}

impl SkillReloadScope {
    pub fn path(&self) -> &Path {
        match self {
            Self::Managed(path) | Self::User(path) | Self::Project(path) => path,
        }
    }

    fn is_watched(&self) -> bool {
        !matches!(self, Self::Managed(_))
    }

    fn load_into(&self, manager: &SkillManager) {
        match self {
            Self::Managed(path) => manager.load_scoped(&SkillScopes {
                managed: Some(path.clone()),
                ..SkillScopes::default()
            }),
            Self::User(path) => manager.load_scoped(&SkillScopes {
                user_skills: Some(path.clone()),
                ..SkillScopes::default()
            }),
            Self::Project(path) => manager.load_scoped(&SkillScopes {
                project_skills: Some(path.clone()),
                ..SkillScopes::default()
            }),
        }
    }
}

pub fn session_reload_scopes(config_home: &Path, cwd: &Path) -> Vec<SkillReloadScope> {
    let mut scopes = vec![
        SkillReloadScope::Managed(crate::get_managed_skills_path()),
        SkillReloadScope::User(config_home.join("skills")),
    ];
    scopes.extend(
        crate::project_skill_dirs_up_to_home(cwd)
            .into_iter()
            .map(SkillReloadScope::Project),
    );
    scopes
}

/// Debounce interval for skill file changes (300 ms).
const SKILL_DEBOUNCE_MS: u64 = 300;

/// Event emitted when skill files change.
#[derive(Debug, Clone, Default)]
pub struct SkillsChanged {
    /// Paths of files that changed (for diagnostics).
    pub changed_paths: Vec<PathBuf>,
    /// Hook declarations from the pending skill set:
    /// `(skill_name, hooks_json)`. Populated before the live manager is
    /// mutated so subscribers can run blocking ConfigChange hooks first.
    ///
    /// Triggers both skill reload AND `hooksConfigSnapshot` invalidation.
    /// The snapshot refresh is the subscriber's responsibility — this field
    /// carries the
    /// minimum data needed (opaque hook JSON) for coco-hooks to rebuild
    /// its skill-scope entries.
    pub skill_hook_declarations: Vec<(String, serde_json::Value)>,
}

/// Watches skill directories and reloads the [`SkillManager`] on
/// changes. Debounced file watcher that triggers skill reload.
///
/// ## Lifecycle
///
/// The watcher's background tasks live inside the wrapped
/// [`FileWatcher`]; when the last `Arc<SkillChangeDetector>` is dropped,
/// the watcher (and its notify thread) drops with it. Callers hold the
/// `Arc` in a guard binding for the session lifetime.
pub struct SkillChangeDetector {
    /// Wrapped generic watcher — owns the OS-event pump, throttle
    /// timer, and post-debounce broadcast channel. The change-tx below
    /// is downstream of this: a tokio task lifts each debounced
    /// `SkillsChanged` into a disk scan then re-broadcasts so
    /// subscribers see the enriched form (`skill_hook_declarations`
    /// populated) before the live manager is changed.
    _inner: FileWatcher<SkillsChanged>,
    /// Shared skill manager that gets reloaded on changes.
    manager: Arc<SkillManager>,
    /// Directories being watched.
    watched_dirs: Vec<PathBuf>,
    /// Disk scopes rebuilt after each debounced change.
    reload_scopes: Vec<SkillReloadScope>,
    /// Broadcast sender for change notifications (pre-reload —
    /// `skill_hook_declarations` is filled in).
    change_tx: broadcast::Sender<SkillsChanged>,
}

impl SkillChangeDetector {
    /// Create a new detector watching the given directories.
    ///
    /// The detector immediately subscribes to filesystem events and
    /// will reload the [`SkillManager`] when `.md` files change.
    /// Returns an `Arc<Self>` so the caller can hold a guard binding
    /// to control lifetime.
    pub fn new(
        manager: Arc<SkillManager>,
        reload_scopes: Vec<SkillReloadScope>,
    ) -> crate::Result<Arc<Self>> {
        let skill_dirs: Vec<PathBuf> = reload_scopes
            .iter()
            .filter(|scope| scope.is_watched())
            .map(|scope| scope.path().to_path_buf())
            .collect();
        let inner = FileWatcherBuilder::new()
            .throttle_interval(Duration::from_millis(SKILL_DEBOUNCE_MS))
            .build(classify, merge)
            .map_err(|e| crate::SkillsError::generic(format!("file-watch build failed: {e}")))?;

        for dir in &skill_dirs {
            // `try_watch` already returns Ok(()) for non-existent paths
            // — no pre-check needed.
            inner.watch(dir.clone(), RecursiveMode::Recursive);
        }

        // Bridge: each debounced `SkillsChanged` from the FileWatcher
        // triggers a fresh skill scan, then we re-broadcast on
        // `change_tx` with hook declarations populated. The live
        // manager is mutated by the app layer after blocking hooks pass.
        let (change_tx, _) = broadcast::channel(32);
        let change_tx_clone = change_tx.clone();
        let reload_scopes_clone = reload_scopes.clone();
        let mut rx = inner.subscribe();
        tokio::spawn(async move {
            while let Ok(mut event) = rx.recv().await {
                tracing::info!(
                    paths = ?event.changed_paths,
                    "skill files changed, scanning"
                );
                let new_skills = load_scoped_skills(&reload_scopes_clone);

                event.skill_hook_declarations = new_skills
                    .iter()
                    .filter_map(|s| s.hooks.as_ref().map(|h| (s.name.clone(), h.clone())))
                    .collect();

                let _ = change_tx_clone.send(event);
            }
        });

        Ok(Arc::new(SkillChangeDetector {
            _inner: inner,
            manager,
            watched_dirs: skill_dirs,
            reload_scopes,
            change_tx,
        }))
    }

    /// Subscribe to skill change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<SkillsChanged> {
        self.change_tx.subscribe()
    }

    /// Reload the live manager from disk after the caller has accepted
    /// the pending change event.
    pub fn reload_now(&self) -> usize {
        let new_skills = load_scoped_skills(&self.reload_scopes);
        self.manager.reload_disk_skills(new_skills);
        self.manager.reset_announcements();
        self.manager.len()
    }

    /// Get a reference to the managed [`SkillManager`].
    pub fn manager(&self) -> &Arc<SkillManager> {
        &self.manager
    }

    /// Get the watched directories.
    pub fn watched_dirs(&self) -> &[PathBuf] {
        &self.watched_dirs
    }

    pub fn reload_scopes(&self) -> &[SkillReloadScope] {
        &self.reload_scopes
    }
}

fn load_scoped_skills(reload_scopes: &[SkillReloadScope]) -> Vec<SkillDefinition> {
    let reloaded = SkillManager::new();
    crate::bundled::register_bundled(&reloaded);
    for scope in reload_scopes {
        scope.load_into(&reloaded);
    }
    reloaded
        .all_including_conditional()
        .into_iter()
        .map(|skill| (*skill).clone())
        .collect()
}

// ─── classify + merge (testable, extracted from build()) ────────────────

fn classify(event: &coco_file_watch::Event) -> Option<SkillsChanged> {
    let md_paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .cloned()
        .collect();
    if md_paths.is_empty() {
        return None;
    }
    Some(SkillsChanged {
        changed_paths: md_paths,
        skill_hook_declarations: Vec::new(),
    })
}

fn merge(mut acc: SkillsChanged, new: SkillsChanged) -> SkillsChanged {
    acc.changed_paths.extend(new.changed_paths);
    // `skill_hook_declarations` is populated post-reload by the bridge
    // task, so merging two raw classify outputs always leaves it empty.
    acc
}
