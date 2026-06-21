//! Resolve the memory directory for a project.
//!
//! Resolution chain:
//!   1. `COCO_MEMORY_PATH_OVERRIDE` env override (operator)
//!   2. `COCO_REMOTE_MEMORY_DIR` env (CCR / swarm leader)
//!   3. `settings.json` `autoMemoryDirectory`
//!   4. `<config_home>/projects/<sanitized-canonical-git-root>/memory/`
//!
//! Steps (1)-(3) are folded into `coco_config::MemoryConfig::resolve`
//! by the time we see the runtime adapter — its `directory: Option<PathBuf>`
//! already represents the result of those overrides. This module
//! handles step (4): the default layout under the config home, anchored
//! to the **canonical** git root so worktrees of the same repo share
//! one memory dir.
//!
//! The slug computation lives in [`coco_paths::ProjectSlug`] /
//! [`coco_paths::ProjectPaths`] — see that crate for the
//! `[^a-zA-Z0-9]` + NFC + 200-byte djb2-hash algorithm.
//! Prior to that consolidation this file held its own buggy variant.

use std::path::Path;
use std::path::PathBuf;

/// Memory directory layout for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDir {
    /// Personal directory.
    pub personal: PathBuf,
    /// Team subdirectory (`personal/team/`). Always derivable but
    /// pre-computed for callsite ergonomics.
    pub team: PathBuf,
}

impl MemoryDir {
    /// Resolve from a project root.
    ///
    /// `override_dir` wins outright (settings + env layers already
    /// merged by `coco_config::MemoryConfig::resolve`). Otherwise the
    /// default layout is `<config_home>/projects/<sanitized>/memory/`,
    /// where `sanitized` is derived from the **canonical** git root so
    /// linked worktrees share one memory dir. When `project_root`
    /// isn't inside a git repo, fall back to the path itself.
    pub fn resolve(config_home: &Path, project_root: &Path, override_dir: Option<&Path>) -> Self {
        let personal = match override_dir {
            Some(custom) => custom.to_path_buf(),
            None => {
                let canonical = coco_git::find_canonical_git_root(project_root)
                    .unwrap_or_else(|| project_root.to_path_buf());
                let project_paths =
                    coco_paths::ProjectPaths::new(config_home.to_path_buf(), &canonical);
                project_paths.memory_dir()
            }
        };
        let team = personal.join("team");
        Self { personal, team }
    }

    /// Path to the personal `MEMORY.md` index.
    pub fn personal_index(&self) -> PathBuf {
        self.personal.join(crate::store::ENTRYPOINT_NAME)
    }

    /// Path to the team `MEMORY.md` index.
    pub fn team_index(&self) -> PathBuf {
        self.team.join(crate::store::ENTRYPOINT_NAME)
    }
}

#[cfg(test)]
#[path = "resolve.test.rs"]
mod tests;
