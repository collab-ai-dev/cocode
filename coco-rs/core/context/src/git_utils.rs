//! Git utility functions for context assembly.

use std::path::Path;
use std::process::Command;

/// Get a concise git diff summary for the working directory.
pub fn get_git_diff_summary(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["diff", "--stat"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}

/// Get the current branch name.
pub fn get_current_branch(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the main/default branch name.
pub fn get_main_branch(cwd: &Path) -> String {
    for branch in &["main", "master", "develop"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(cwd)
            .output();
        if output.is_ok_and(|o| o.status.success()) {
            return branch.to_string();
        }
    }
    "main".to_string()
}

/// Get recent commits (for context).
pub fn get_recent_commits(cwd: &Path, count: i32) -> Vec<String> {
    let output = Command::new("git")
        .args(["log", "--oneline", &format!("-{count}")])
        .current_dir(cwd)
        .output();

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Check if a path is tracked by git.
pub fn is_git_tracked(cwd: &Path, file_path: &str) -> bool {
    Command::new("git")
        .args(["ls-files", "--error-unmatch", file_path])
        .current_dir(cwd)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Get the git root directory.
pub fn get_git_root(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Check if the repo has uncommitted changes.
pub fn has_uncommitted_changes(cwd: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Create a git worktree for agent isolation.
pub fn create_worktree(cwd: &Path, branch: &str, path: &Path) -> crate::Result<()> {
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch, &path.to_string_lossy()])
        .current_dir(cwd)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::ContextError::git_failed(format!(
            "git worktree add failed: {stderr}"
        )));
    }
    Ok(())
}

/// Remove a git worktree.
pub fn remove_worktree(cwd: &Path, path: &Path, force: bool) -> crate::Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);

    let output = Command::new("git").args(&args).current_dir(cwd).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::ContextError::git_failed(format!(
            "git worktree remove failed: {stderr}"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "git_utils.test.rs"]
mod tests;
