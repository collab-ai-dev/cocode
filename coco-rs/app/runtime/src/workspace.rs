//! Session workspace path resolution: the per-session cwd, its resolved
//! project root, and storage anchors.
//!
//! Owned here because the project root derived by [`resolve_project_root`] is
//! the key under which [`crate::ProjectRegistry`] caches services; session
//! storage and project-service lookup must derive it identically or they
//! diverge.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_config::env;
use coco_config::env::EnvKey;
use coco_config::global_config;
use coco_paths::ProjectPaths;
use coco_paths::RuntimePaths;

/// Resolved path anchors for one session.
///
/// `cwd` is the session's working directory. `project_root` is the
/// `ProjectServices` cache key: the git worktree root when available, else
/// `cwd`. `storage_paths` intentionally preserves the existing transcript /
/// memory layout anchor, which is still keyed by the session cwd.
#[derive(Debug, Clone)]
pub struct SessionWorkspace {
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    pub storage_paths: Arc<ProjectPaths>,
}

impl SessionWorkspace {
    pub fn resolve(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let project_root = resolve_project_root(&cwd);
        let storage_paths = project_paths(&cwd);
        Self {
            cwd,
            project_root,
            storage_paths,
        }
    }
}

/// Resolve settings-layer roots for a session cwd.
///
/// Project settings follow the resolved project root; local settings remain
/// scoped to the session cwd.
pub fn settings_roots_for_cwd(cwd: &Path) -> coco_config::SettingsRoots {
    let workspace = SessionWorkspace::resolve(cwd.to_path_buf());
    coco_config::SettingsRoots::new(workspace.project_root, workspace.cwd)
}

/// Resolve the project root used by project-scoped services.
///
/// This intentionally returns the worktree root, not the canonical shared git
/// directory root, so linked worktrees can host independent project services.
pub fn resolve_project_root(cwd: &Path) -> PathBuf {
    git_root_for(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

/// Resolve runtime path roots at the CLI/context boundary.
///
/// `coco-paths` deliberately does not read process env. This folds
/// `COCO_REMOTE_MEMORY_DIR` into the project-scoped path layout while leaving
/// config-home artifacts on `global_config::config_home()`.
pub fn runtime_paths() -> RuntimePaths {
    let config_home = global_config::config_home();
    let memory_base_override = env::var_os(EnvKey::CocoRemoteMemoryDir).map(PathBuf::from);
    RuntimePaths::new(config_home, memory_base_override)
}

/// Build [`ProjectPaths`] for `cwd`.
///
/// Returns an `Arc<ProjectPaths>` so callers can cheaply share one instance
/// across session/transcript subsystems. `cwd` should already be canonicalised
/// by the caller; session paths intentionally keep the worktree/project cwd so
/// linked worktrees get distinct transcript project dirs.
pub fn project_paths(cwd: &Path) -> Arc<ProjectPaths> {
    Arc::new(runtime_paths().project_paths(cwd))
}

/// Resolve the worktree's own git root (the directory containing the `.git`
/// file or directory) starting at `cwd`. Returns `None` if `cwd` is not inside
/// any git tree. This is distinct from [`coco_git::find_canonical_git_root`],
/// which collapses worktrees onto the main repo via `--git-common-dir`.
pub fn git_root_for(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}
