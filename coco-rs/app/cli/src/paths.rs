//! Path helpers shared by binary subcommand handlers and library
//! bootstrap code.
//!
//! Centralizes path construction that was previously duplicated across
//! `main.rs`, `tui_runner.rs`, and `run_sdk_mode`: the sessions
//! directory, the agent search paths, and the output-style directories.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_config::global_config;
use coco_paths::ProjectPaths;

/// `~/.coco/sessions` — legacy flat session directory.
///
/// **DEPRECATED** for new code. The current layout puts session files
/// under `~/.coco/projects/{sanitized_cwd}/{session_id}.jsonl` — use
/// [`project_paths`] to construct paths in the new layout.
///
/// This function is retained until every consumer migrates to
/// `ProjectPaths`. TODO: remove once the migration in
/// `docs/coco-rs/session-projects-migration.md` is complete.
pub fn sessions_dir() -> PathBuf {
    global_config::config_home().join("sessions")
}

/// Build [`ProjectPaths`] for `cwd`.
///
/// Returns an `Arc<ProjectPaths>` so callers can cheaply share one
/// instance across session/transcript subsystems.
///
/// `cwd` should already be canonicalised by the caller. Unlike the memory
/// layer, session paths intentionally keep the worktree/project cwd so linked
/// worktrees get distinct transcript project dirs, mirroring the TS runtime.
pub fn project_paths(cwd: &Path) -> Arc<ProjectPaths> {
    let memory_base = global_config::config_home();
    Arc::new(ProjectPaths::new(memory_base, cwd))
}

/// `~/.coco/output-styles` — user-scope output style markdown dir.
///
/// [`OutputStyleManagerBuilder`] also honors managed and project sources
/// — see [`output_style_dirs`].
///
/// [`OutputStyleManagerBuilder`]: coco_output_styles::manager::OutputStyleManagerBuilder
/// [`output_style_dirs`]: self::output_style_dirs
pub fn user_output_style_dir() -> PathBuf {
    global_config::config_home().join("output-styles")
}

/// `<cwd>/.coco/output-styles` — direct project output style dir.
pub fn project_output_style_dir(cwd: &Path) -> PathBuf {
    cwd.join(".coco").join("output-styles")
}

/// Project output-style dirs from most-specific to least-specific.
///
/// The walk starts at `cwd`, checks each `.coco/output-styles`
/// directory, and stops after the git root when inside a repository; if
/// not in git, it stops at the user's home directory or filesystem root.
/// Linked worktrees fall back to the canonical repository copy when the
/// worktree root does not have `.coco/output-styles` checked out.
pub fn project_output_style_dirs(cwd: &Path) -> Vec<PathBuf> {
    project_coco_subdirs_up_to_home("output-styles", cwd)
}

/// Cross-platform managed/policy directory for output styles. Mirrors
/// [`coco_skills::get_managed_skills_path`] but for `output-styles`.
pub fn managed_output_style_dir() -> PathBuf {
    global_config::managed_settings_path()
        .parent()
        .map(|dir| dir.join("output-styles"))
        .unwrap_or_else(|| PathBuf::from("/etc/coco/output-styles"))
}

/// Directory list for output styles in priority order
/// (lowest to highest): user → project → managed.
///
/// Returned for the SDK `available_output_styles` `discover_*` legacy
/// path; new code prefers
/// [`coco_output_styles::OutputStyleManager::builder`] which accepts
/// each source separately so priority is enforced explicitly.
pub fn output_style_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(user_output_style_dir());
    dirs.extend(project_output_style_dirs(cwd));
    dirs.push(managed_output_style_dir());
    dirs
}

/// Standard CLI agent search paths: `~/.coco/agents` (user) plus
/// `<cwd>/.coco/agents` (project).
///
/// **Worktree fallback**: when `cwd` resolves into a linked git worktree
/// whose `.coco/agents/` is empty (or not checked out), we additionally
/// search the canonical (main) repo's `.coco/agents/`. The fallback only
/// fires when the canonical root differs from the worktree's git root
/// **and** the worktree dir is missing — a `git worktree add` checks out
/// the full tree, so the shared case (worktree already has the same agent
/// files) skips the fallback to keep precedence stable.
pub fn standard_agent_search_paths(
    config_home: &Path,
    cwd: &Path,
) -> coco_subagent::definition_store::AgentSearchPaths {
    let mut project_dirs = vec![cwd.join(".coco").join("agents")];

    // Push the canonical-repo fallback when applicable. Errors / no-git
    // states are treated as "no fallback needed" — the loader degrades
    // gracefully on missing dirs.
    if let Some(canonical_root) = coco_git::find_canonical_git_root(cwd) {
        let worktree_agents_dir = cwd.join(".coco").join("agents");
        let worktree_root = git_root_for(cwd);
        let worktree_has_agents = std::fs::metadata(&worktree_agents_dir)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let canonical_agents_dir = canonical_root.join(".coco").join("agents");
        // Only add the canonical-root copy when:
        // 1. cwd is inside a worktree distinct from the canonical root, AND
        // 2. the worktree's own .coco/agents/ is missing or empty.
        // Same-root cases (cwd == canonical_root, or worktree already
        // has agent files) keep the original single-entry shape.
        if worktree_root.as_deref() != Some(canonical_root.as_path())
            && !worktree_has_agents
            && canonical_agents_dir.is_dir()
            && !project_dirs.iter().any(|p| p == &canonical_agents_dir)
        {
            project_dirs.push(canonical_agents_dir);
        }
    }

    coco_subagent::definition_store::AgentSearchPaths {
        user_dir: Some(config_home.join("agents")),
        project_dirs,
        plugin_dirs: plugin_agent_dirs(config_home, cwd),
        ..coco_subagent::definition_store::AgentSearchPaths::empty()
    }
}

/// Resolve the session's enabled plugins to namespaced agent directories.
/// The discovery (which dirs) lives in `coco_plugins::plugin_agent_dirs`; this
/// wraps each `(name, dir)` in the subagent loader's `PluginAgentDir`.
fn plugin_agent_dirs(
    config_home: &Path,
    cwd: &Path,
) -> Vec<coco_subagent::definition_store::PluginAgentDir> {
    let plugins = coco_plugins::load_enabled_plugins(config_home, cwd);
    coco_plugins::plugin_agent_dirs(&plugins)
        .into_iter()
        .map(
            |(plugin_name, dir)| coco_subagent::definition_store::PluginAgentDir {
                plugin_name,
                dir,
            },
        )
        .collect()
}

/// Resolve the worktree's own git root (the directory containing the
/// `.git` file or directory) starting at `cwd`. Returns `None` if `cwd`
/// is not inside any git tree. This is distinct from
/// [`coco_git::find_canonical_git_root`], which collapses worktrees
/// onto the main repo via `--git-common-dir`.
fn git_root_for(cwd: &Path) -> Option<PathBuf> {
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

fn project_coco_subdirs_up_to_home(subdir: &str, cwd: &Path) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let git_root = git_root_for(cwd);
    let mut current = cwd.to_path_buf();
    let mut dirs = Vec::new();

    loop {
        if home.as_deref().is_some_and(|h| same_path(&current, h)) {
            break;
        }

        let candidate = current.join(".coco").join(subdir);
        if candidate.is_dir() {
            dirs.push(candidate);
        }

        if git_root
            .as_deref()
            .is_some_and(|root| same_path(&current, root))
        {
            break;
        }

        if !current.pop() {
            break;
        }
    }

    add_worktree_canonical_fallback(subdir, cwd, &git_root, &mut dirs);
    dirs
}

fn add_worktree_canonical_fallback(
    subdir: &str,
    cwd: &Path,
    git_root: &Option<PathBuf>,
    dirs: &mut Vec<PathBuf>,
) {
    let Some(canonical_root) = coco_git::find_canonical_git_root(cwd) else {
        return;
    };
    if git_root.as_deref() == Some(canonical_root.as_path()) {
        return;
    }

    let worktree_has_subdir = git_root
        .as_ref()
        .map(|root| root.join(".coco").join(subdir))
        .is_some_and(|worktree_subdir| dirs.iter().any(|dir| same_path(dir, &worktree_subdir)));
    if worktree_has_subdir {
        return;
    }

    let canonical_subdir = canonical_root.join(".coco").join(subdir);
    if !dirs.iter().any(|dir| same_path(dir, &canonical_subdir)) {
        dirs.push(canonical_subdir);
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

#[cfg(test)]
#[path = "paths.test.rs"]
mod tests;
