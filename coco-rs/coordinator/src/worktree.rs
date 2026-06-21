//! Agent worktree manager — Phase 6, Workstream C.
//!
//! The parent's cwd is **not** changed — subagents see the worktree
//! via `ToolUseContext::cwd_override` (explicit field propagation is the
//! substitute for an async-local cwd).
//!
//! # Scope for the first Rust slice
//!
//! Implemented:
//! - `git worktree add -B <branch> <path>` against canonical git root.
//! - `hasWorktreeChanges`: dirty working tree (`git status --porcelain`) OR
//!   new commits since creation (`rev-list --count <head>..HEAD`).
//! - `git worktree remove --force` + `git branch -D`.
//! - Post-creation setup: settings.local.json copy + git core.hooksPath config.
//! - Periodic stale-worktree sweep (`cleanup_stale`):
//!   age + clean-tree + remote-reachable (unpushed-commit fail-close) +
//!   `git worktree prune`.
//!
//! Deferred (out of scope per plan review):
//! - Hook-based VCS (`WorktreeCreate` hook).
//! - Commit-attribution prepare-commit-msg hook.
//! - Resume metadata.
//!
//! # Canonical git root
//!
//! Agent worktrees always land in the **canonical** repo's
//! `.coco/worktrees/` dir, even when spawned from inside a session
//! worktree. The canonical root is resolved via
//! [`AgentWorktreeManager::canonical_git_root`] at manager construction
//! so subagent worktrees never nest.
//!
//! # Cleanup-on-change policy
//!
//! If the child agent made no changes (staged, unstaged, or
//! untracked), the worktree is removed after the agent completes. If
//! changes exist, the worktree is **kept** on disk for the user to
//! inspect — see the `kept` variant of [`WorktreeCleanupOutcome`].

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

/// Agent-scoped worktree session. Carries everything needed for
/// cleanup + cwd override on the child agent.
#[derive(Debug, Clone)]
pub struct AgentWorktreeSession {
    /// Absolute worktree path — becomes the child's `cwd_override`.
    pub path: PathBuf,
    /// Temporary branch created by `git worktree add -B`. Deleted on
    /// successful cleanup.
    pub branch: String,
    /// HEAD commit at creation time. Used by `cleanup_if_unchanged`
    /// to detect whether the agent modified anything.
    pub head_commit: String,
    /// Main repo's canonical git root — NOT the worktree path.
    /// Required because `git worktree remove` must be invoked from
    /// the main repo, not the worktree being deleted.
    pub git_root: PathBuf,
}

/// Outcome returned by [`AgentWorktreeManager::cleanup_if_unchanged`].
///
/// Empty result = removed, populated = kept on disk.
#[derive(Debug, Clone)]
pub enum WorktreeCleanupOutcome {
    /// Worktree was removed because the child agent made no changes.
    Removed,
    /// Worktree was kept on disk. Either the child made changes, the
    /// cleanup subprocess failed, or the agent crashed mid-execution.
    Kept {
        path: PathBuf,
        branch: String,
        reason: KeptReason,
    },
}

/// Why a worktree was kept rather than removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptReason {
    /// Child agent made changes (staged / unstaged / untracked).
    HasChanges,
    /// `git worktree remove` or `git branch -D` failed. Worktree
    /// path may or may not still exist — the caller should warn but
    /// not block the session.
    CleanupFailed,
}

/// Agent worktree creation / cleanup errors. Surfaced to the agent
/// runtime as a tool-result error when the initial create fails;
/// cleanup errors are downgraded to `Kept { reason: CleanupFailed }`
/// so the session doesn't abort.
///
/// Uses snafu + virtual stack traces + [`ErrorExt`] per the project
/// root `CLAUDE.md` "Error Handling" rule for core/root crates. The
/// foreign source errors (`coco_git::GitToolingError`, `std::io::Error`)
/// don't implement `coco_error::StackError`, so they're flattened to
/// their string representation at the boundary — the cause text is
/// preserved in the variant message while the variant itself stays
/// `StackError`-compatible (which `ErrorExt` requires).
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
#[allow(clippy::enum_variant_names)]
pub enum WorktreeError {
    #[snafu(display("not in a git repository (no canonical git root resolvable from {path:?})"))]
    NotInRepo {
        path: PathBuf,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("invalid worktree slug {slug:?}: {reason}"))]
    InvalidSlug {
        slug: String,
        reason: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("git subprocess failed: {stderr}"))]
    GitFailed {
        stderr: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("git tooling error: {message}"))]
    Git {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("io error during worktree setup: {message}"))]
    Io {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for WorktreeError {
    fn status_code(&self) -> StatusCode {
        // Most worktree failures are local environment / config issues
        // — not retryable. The user typically needs to resolve repo
        // state (initialize git, free a slug, fix permissions) before
        // retrying. `Io` covers transient FS failures too, but they're
        // rare enough that auto-retry would mask real bugs.
        match self {
            Self::NotInRepo { .. } | Self::InvalidSlug { .. } => StatusCode::InvalidArguments,
            Self::GitFailed { .. } | Self::Git { .. } => StatusCode::Internal,
            Self::Io { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// Restore historical `?` ergonomics by flattening foreign source errors
// to their `Display` string at the conversion boundary. We deliberately
// drop the live source pointer so the variant satisfies
// `StackError`-only sources (which `stack_trace_debug` requires) — the
// cause text survives in `message`, threaded through Display.
impl From<coco_git::GitToolingError> for WorktreeError {
    fn from(source: coco_git::GitToolingError) -> Self {
        Self::Git {
            message: source.to_string(),
            location: Location::default(),
        }
    }
}

impl From<std::io::Error> for WorktreeError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            message: source.to_string(),
            location: Location::default(),
        }
    }
}

/// Configuration for optional post-creation setup behaviors.
///
/// All fields are opt-in. The defaults (`Default::default()`) give the
/// minimum setup: settings.local.json copy + core.hooksPath config only.
/// Enable `symlink_directories` to avoid duplicating large dirs like
/// `node_modules` across worktrees.
#[derive(Debug, Clone, Default)]
pub struct AgentWorktreeConfig {
    /// Directories to symlink from the main repo into each new
    /// worktree. Relative to the main repo root.
    ///
    /// Configured via `settings.worktree.symlinkDirectories`. Typical
    /// values: `["node_modules", "target", ".venv"]`. Missing source dirs
    /// are silently skipped.
    pub symlink_directories: Vec<PathBuf>,
}

/// Agent worktree manager.
///
/// Constructed once per session from a resolved canonical git root.
/// All `create_for` and `cleanup_if_unchanged` calls operate against
/// that root — nested spawns from inside a session worktree still
/// land their worktrees in the main repo's `.coco/worktrees/`.
pub struct AgentWorktreeManager {
    canonical_git_root: PathBuf,
    config: AgentWorktreeConfig,
}

impl AgentWorktreeManager {
    /// Build a manager from an explicit canonical git root.
    /// Callers typically resolve this at session bootstrap via
    /// [`Self::discover_from_cwd`].
    pub fn new(canonical_git_root: PathBuf) -> Self {
        Self {
            canonical_git_root,
            config: AgentWorktreeConfig::default(),
        }
    }

    /// Install post-creation setup configuration. Builder-style so
    /// the CLI can plumb `settings.worktree.symlinkDirectories` and
    /// other advanced options without growing `new()`.
    pub fn with_config(mut self, config: AgentWorktreeConfig) -> Self {
        self.config = config;
        self
    }

    /// Discover the canonical git root from `cwd`, falling back to
    /// `NotInRepo` if the directory isn't inside a git repo.
    ///
    /// Resolves symlinks and walks to the main repo, not the nearest
    /// `.git` — so a session spawned inside a worktree still sees the
    /// main repo as its root.
    pub fn discover_from_cwd(cwd: &Path) -> Result<Self, WorktreeError> {
        // Use `git rev-parse --git-common-dir` for canonical resolution.
        // `--git-common-dir` returns the SHARED .git directory across
        // worktrees (main repo's .git), not the current worktree's
        // .git/worktrees/<name> link.
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--git-common-dir"])
            .output()?;
        if !output.status.success() {
            return Err(WorktreeError::NotInRepo {
                path: cwd.to_path_buf(),
                location: Location::default(),
            });
        }
        let common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // The common dir is usually "<root>/.git"; the repo root is its parent.
        let common_dir_path = PathBuf::from(&common_dir);
        let absolute = if common_dir_path.is_absolute() {
            common_dir_path
        } else {
            cwd.join(&common_dir_path)
        };
        let canonical = absolute.canonicalize()?;
        let root = canonical
            .parent()
            .ok_or_else(|| WorktreeError::NotInRepo {
                path: cwd.to_path_buf(),
                location: Location::default(),
            })?
            .to_path_buf();
        Ok(Self::new(root))
    }

    /// The canonical git root this manager operates against.
    pub fn canonical_git_root(&self) -> &Path {
        &self.canonical_git_root
    }

    /// Create a fresh agent worktree with the given slug.
    ///
    /// Slug format: `agent-<first-8-hex>` derived from the agent id.
    /// Validated here to reject path separators + shell metacharacters.
    ///
    /// Side effects (post-creation setup):
    /// - Copy `.coco/settings.local.json` into the worktree.
    /// - Configure `core.hooksPath` to point at the main repo's hooks
    ///   (so husky / custom hooks resolve correctly).
    pub fn create_for(&self, slug: &str) -> Result<AgentWorktreeSession, WorktreeError> {
        validate_slug(slug)?;

        let worktree_path = self
            .canonical_git_root
            .join(".coco")
            .join("worktrees")
            .join(slug);
        let branch = format!("claude/{slug}");

        // Ensure parent dir exists so `git worktree add` doesn't
        // fail on a fresh repo that's never had a worktree before.
        if let Some(parent) = worktree_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // `-B` creates-or-resets the branch; an existing agent worktree
        // can be reused.
        let add_output = Command::new("git")
            .arg("-C")
            .arg(&self.canonical_git_root)
            .args(["worktree", "add", "-B", &branch])
            .arg(&worktree_path)
            .output()?;
        if !add_output.status.success() {
            return Err(WorktreeError::GitFailed {
                stderr: String::from_utf8_lossy(&add_output.stderr).into_owned(),
                location: Location::default(),
            });
        }

        let head_commit = get_head_commit(&worktree_path)?;

        // Best-effort post-creation setup. Failures here are
        // non-fatal — a worktree without settings.local still works,
        // just with reduced per-project settings.
        let _ = copy_settings_local(&self.canonical_git_root, &worktree_path);
        let _ = configure_hooks_path(&self.canonical_git_root, &worktree_path);
        // Symlink configured directories (e.g. node_modules) from the
        // main repo per the `symlinkDirectories` setting.
        if !self.config.symlink_directories.is_empty() {
            let _ = symlink_directories(
                &self.canonical_git_root,
                &worktree_path,
                &self.config.symlink_directories,
            );
        }

        Ok(AgentWorktreeSession {
            path: worktree_path,
            branch,
            head_commit,
            git_root: self.canonical_git_root.clone(),
        })
    }

    /// Whether hook-based worktree creation is available. Returns
    /// `true` when the provided hook registry contains at least one
    /// `WorktreeCreate` handler.
    ///
    /// When `true`, the caller may route worktree creation through the
    /// hook runner to support non-git VCS (Jujutsu, Mercurial, etc.).
    /// When `false`, falls back to `git worktree add`.
    ///
    /// This method does **not** execute any hooks — it only checks
    /// registration. Full hook-based worktree routing is deferred
    /// (see module doc "Scope for the first Rust slice").
    pub fn has_worktree_create_hook(registry: &coco_hooks::HookRegistry) -> bool {
        !registry
            .find_matching(coco_types::HookEventType::WorktreeCreate, None)
            .is_empty()
    }

    /// Sweep stale agent worktrees — background-safe cleanup.
    ///
    /// Scans `.coco/worktrees/agent-*` under the canonical root
    /// and removes directories whose last-modified time is older
    /// than `older_than`. Used for cases where a prior session
    /// crashed before `cleanup_if_unchanged` could run (parent
    /// killed by ESC/Ctrl+C, crash, lost connection, etc.).
    ///
    /// Uses a 30-day threshold by default. Returns the number of
    /// worktrees removed.
    ///
    /// Silently skips worktrees that still have changes — user's
    /// work is preserved even if the agent metadata is lost.
    ///
    /// This is a best-effort cleanup; all errors are swallowed so
    /// a stuck worktree doesn't block session startup.
    pub fn cleanup_stale(&self, older_than: std::time::Duration) -> usize {
        let worktrees_dir = self.canonical_git_root.join(".coco").join("worktrees");
        let entries = match std::fs::read_dir(&worktrees_dir) {
            Ok(e) => e,
            Err(_) => return 0, // dir doesn't exist — nothing to sweep.
        };

        let now = std::time::SystemTime::now();
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Match exactly the pattern `agent-<8-hex-chars>` used
            // by `create_for`. Refuse to sweep user-named
            // worktrees (e.g. `wt-myfeature`).
            if !is_agent_slug(&name_str) {
                continue;
            }
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = metadata.modified().unwrap_or(now);
            let age = now.duration_since(mtime).unwrap_or_default();
            if age < older_than {
                continue;
            }
            // Preserve any worktree with a dirty tree OR commits not yet on a
            // remote. The stale path has no original head to diff against (the
            // session metadata is gone after a crash/kill), so committed work
            // is guarded by the unpushed-commit check rather than head..HEAD.
            // Fail-closed: a git error keeps the worktree.
            if has_worktree_changes(&path, "").unwrap_or(true)
                || has_unpushed_commits(&path).unwrap_or(true)
            {
                continue; // preserve user's work.
            }
            // Resolve the branch name from the slug.
            let branch = format!("claude/{name_str}");
            let session = AgentWorktreeSession {
                path: path.clone(),
                branch,
                head_commit: String::new(),
                git_root: self.canonical_git_root.clone(),
            };
            if let WorktreeCleanupOutcome::Removed = self.cleanup_if_unchanged(session) {
                removed += 1;
            }
        }
        if removed > 0 {
            // Drop git's internal registry entries for the now-deleted dirs.
            let _ = git_stdout(&self.canonical_git_root, &["worktree", "prune"]);
        }
        removed
    }

    /// Remove the worktree if the child agent made no changes; keep
    /// it on disk otherwise.
    ///
    /// "Changes" means a dirty working tree (`git status --porcelain`) OR
    /// commits made since `session.head_commit` — a clean tree can still hold
    /// committed work that the force-remove + `branch -D` below would destroy.
    pub fn cleanup_if_unchanged(&self, session: AgentWorktreeSession) -> WorktreeCleanupOutcome {
        let has_changes = match has_worktree_changes(&session.path, &session.head_commit) {
            Ok(b) => b,
            Err(_) => {
                // Can't determine — err on the side of keeping for
                // user inspection; a failed status query defaults to "keep".
                return WorktreeCleanupOutcome::Kept {
                    path: session.path,
                    branch: session.branch,
                    reason: KeptReason::CleanupFailed,
                };
            }
        };
        if has_changes {
            return WorktreeCleanupOutcome::Kept {
                path: session.path,
                branch: session.branch,
                reason: KeptReason::HasChanges,
            };
        }

        // Remove worktree from main repo (not from inside the
        // worktree — `git` would refuse).
        let remove_output = Command::new("git")
            .arg("-C")
            .arg(&session.git_root)
            .args(["worktree", "remove", "--force"])
            .arg(&session.path)
            .output();
        let remove_ok = matches!(remove_output, Ok(ref o) if o.status.success());
        if !remove_ok {
            return WorktreeCleanupOutcome::Kept {
                path: session.path,
                branch: session.branch,
                reason: KeptReason::CleanupFailed,
            };
        }

        // Delete the temp branch. Non-fatal if it fails (e.g. branch
        // was detached); TS also swallows errors here.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&session.git_root)
            .args(["branch", "-D", &session.branch])
            .output();

        WorktreeCleanupOutcome::Removed
    }
}

/// Match exactly the `agent-<8-hex-chars>` slug shape used by
/// [`AgentWorktreeManager::create_for`]. This narrow pattern keeps
/// stale-sweep from touching user-named EnterWorktree slugs like
/// `wt-myfeature`.
fn is_agent_slug(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("agent-") else {
        return false;
    };
    rest.len() == 8 && rest.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_slug(slug: &str) -> Result<(), WorktreeError> {
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug {
            slug: slug.into(),
            reason: "empty".into(),
            location: Location::default(),
        });
    }
    for c in slug.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !ok {
            return Err(WorktreeError::InvalidSlug {
                slug: slug.into(),
                reason: format!("invalid character {c:?}"),
                location: Location::default(),
            });
        }
    }
    Ok(())
}

fn get_head_commit(path: &Path) -> Result<String, WorktreeError> {
    Ok(coco_git::get_head_commit(path)?)
}

/// Whether the worktree holds work that must NOT be force-removed: a dirty
/// working tree OR commits made since `head_commit` (the commit it was created
/// from). A clean working tree can still hold committed work that
/// `git worktree remove --force` + `branch -D` would destroy, so the commit
/// check is essential — without it an agent that commits its output loses it.
/// An empty `head_commit` (the stale sweep, where the original head is unknown)
/// skips the commit check; that path guards committed work via
/// [`has_unpushed_commits`].
fn has_worktree_changes(path: &Path, head_commit: &str) -> Result<bool, WorktreeError> {
    if !coco_git::get_uncommitted_changes(path)?.is_empty() {
        return Ok(true);
    }
    has_commits_since(path, head_commit)
}

/// New commits on HEAD since `head_commit` — `git rev-list --count
/// <head>..HEAD` > 0. Empty `head_commit` → `Ok(false)` (check skipped).
fn has_commits_since(path: &Path, head_commit: &str) -> Result<bool, WorktreeError> {
    if head_commit.is_empty() {
        return Ok(false);
    }
    let count = git_stdout(
        path,
        &["rev-list", "--count", &format!("{head_commit}..HEAD")],
    )?;
    Ok(count.trim().parse::<u64>().unwrap_or(0) > 0)
}

/// Any commit on HEAD not reachable from a remote — unpushed work the stale
/// sweep must preserve (it has no original head to diff against).
/// Uses `git rev-list --max-count=1 HEAD --not --remotes`.
fn has_unpushed_commits(path: &Path) -> Result<bool, WorktreeError> {
    let out = git_stdout(
        path,
        &["rev-list", "--max-count=1", "HEAD", "--not", "--remotes"],
    )?;
    Ok(!out.trim().is_empty())
}

/// Run `git -C <cwd> <args>` and return stdout, or a `GitFailed` / `Io` error
/// (callers fail-closed to "keep the worktree" on error).
fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| WorktreeError::Io {
            message: e.to_string(),
            location: Location::default(),
        })?;
    if !out.status.success() {
        return Err(WorktreeError::GitFailed {
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            location: Location::default(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Copies `.coco/settings.local.json` from the main repo into the worktree
/// so child agents inherit local settings (auth tokens, per-project
/// preferences).
fn copy_settings_local(repo_root: &Path, worktree_path: &Path) -> Result<(), WorktreeError> {
    let src = repo_root.join(".coco").join("settings.local.json");
    if !src.exists() {
        return Ok(()); // no local settings to propagate — no-op.
    }
    let dst = worktree_path.join(".coco").join("settings.local.json");
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&src, &dst)?;
    Ok(())
}

/// Configures `core.hooksPath` inside the worktree to point at the main
/// repo's `.husky/` or `.git/hooks/`, so pre-commit / post-commit
/// hooks resolve correctly in the worktree.
fn configure_hooks_path(repo_root: &Path, worktree_path: &Path) -> Result<(), WorktreeError> {
    let husky = repo_root.join(".husky");
    let git_hooks = repo_root.join(".git").join("hooks");
    let hooks_path = if husky.is_dir() {
        husky
    } else if git_hooks.is_dir() {
        git_hooks
    } else {
        return Ok(()); // no hooks dir — nothing to configure.
    };
    let hooks_str = hooks_path.to_string_lossy().into_owned();
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["config", "core.hooksPath", &hooks_str])
        .output()?;
    if !output.status.success() {
        // Log via stderr bubble, but don't fail creation.
        return Err(WorktreeError::GitFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            location: Location::default(),
        });
    }
    Ok(())
}

/// Symlinks configured directories from the main repo into the fresh
/// worktree so large dirs (node_modules, target, .venv) aren't
/// duplicated across every agent's worktree.
///
/// Missing source dirs are silently skipped. Any already-existing entry
/// in the worktree is left alone — a later agent might have populated it
/// and we refuse to clobber.
fn symlink_directories(
    repo_root: &Path,
    worktree_path: &Path,
    dirs: &[PathBuf],
) -> Result<(), WorktreeError> {
    for dir in dirs {
        let src = repo_root.join(dir);
        if !src.exists() {
            continue;
        }
        let dst = worktree_path.join(dir);
        if dst.exists() {
            continue; // refuse to clobber an existing entry.
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Use OS-specific symlink creation. Unix: `symlink`; Windows:
        // `symlink_dir` (requires admin/developer mode). We use the
        // `unix` path as the canonical implementation since coco-rs
        // Windows support goes through WSL where Unix symlinks work.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&src, &dst)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "worktree.test.rs"]
mod tests;
