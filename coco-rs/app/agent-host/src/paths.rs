//! Path helpers shared by binary subcommand handlers and library
//! bootstrap code.
//!
//! Centralizes path construction that was previously duplicated across
//! `main.rs`, `tui_runner.rs`, and `run_sdk_mode`: the sessions
//! directory, the agent search paths, and the output-style directories.
//!
//! Session workspace resolution (`SessionWorkspace`, `resolve_project_root`,
//! `project_paths`, `runtime_paths`, `settings_roots_for_cwd`, `git_root_for`)
//! is owned by `coco-app-runtime` — the project root it derives is the
//! `ProjectServices` cache key — and re-exported here for existing callers.

use std::path::Path;
use std::path::PathBuf;

use coco_config::global_config;

pub use coco_app_runtime::SessionWorkspace;
pub use coco_app_runtime::git_root_for;
pub use coco_app_runtime::project_paths;
pub use coco_app_runtime::resolve_project_root;
pub use coco_app_runtime::runtime_paths;
pub use coco_app_runtime::settings_roots_for_cwd;

/// `config home/output-styles` — user-scope output style markdown dir.
///
/// [`OutputStyleManagerBuilder`] also honors managed and project sources
/// — see [`output_style_dirs`].
///
/// [`OutputStyleManagerBuilder`]: coco_output_styles::manager::OutputStyleManagerBuilder
/// [`output_style_dirs`]: self::output_style_dirs
pub fn user_output_style_dir() -> PathBuf {
    global_config::config_home().join("output-styles")
}

/// `project config dir/output-styles` — direct project output style dir.
pub fn project_output_style_dir(cwd: &Path) -> PathBuf {
    cwd.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("output-styles")
}

/// Project output-style dirs from most-specific to least-specific.
///
/// The walk starts at `cwd`, checks each `project config dir/output-styles`
/// directory, and stops after the git root when inside a repository; if
/// not in git, it stops at the user's home directory or filesystem root.
/// Linked worktrees fall back to the canonical repository copy when the
/// worktree root does not have `project config dir/output-styles` checked out.
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

/// Standard CLI agent search paths: `config home/agents` (user) plus
/// `project config dir/agents` (project).
///
/// **Worktree fallback**: when `cwd` resolves into a linked git worktree
/// whose `project config dir/agents/` is empty (or not checked out), we additionally
/// search the canonical (main) repo's `project config dir/agents/`. The fallback only
/// fires when the canonical root differs from the worktree's git root
/// **and** the worktree dir is missing — a `git worktree add` checks out
/// the full tree, so the shared case (worktree already has the same agent
/// files) skips the fallback to keep precedence stable.
pub fn standard_agent_search_paths(
    config_home: &Path,
    cwd: &Path,
) -> coco_subagent::definition_store::AgentSearchPaths {
    let project_root = resolve_project_root(cwd);
    standard_agent_search_paths_for_project(config_home, cwd, &project_root)
}

pub fn standard_agent_search_paths_for_project(
    config_home: &Path,
    cwd: &Path,
    project_root: &Path,
) -> coco_subagent::definition_store::AgentSearchPaths {
    let plugins = coco_plugins::load_enabled_plugins(config_home, project_root);
    standard_agent_search_paths_with_plugins(config_home, cwd, &plugins)
}

pub fn standard_agent_search_paths_with_plugins(
    config_home: &Path,
    cwd: &Path,
    plugins: &[coco_plugins::loader::LoadedPluginV2],
) -> coco_subagent::definition_store::AgentSearchPaths {
    coco_app_runtime::standard_agent_search_paths_with_plugins(config_home, cwd, plugins)
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

        let candidate = current
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join(subdir);
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
        .map(|root| {
            root.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                .join(subdir)
        })
        .is_some_and(|worktree_subdir| dirs.iter().any(|dir| same_path(dir, &worktree_subdir)));
    if worktree_has_subdir {
        return;
    }

    let canonical_subdir = canonical_root
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join(subdir);
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
