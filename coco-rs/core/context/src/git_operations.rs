//! Extended git operations for tool context.

use std::path::Path;
use std::process::Command;

/// Check if a file is gitignored.
pub fn is_gitignored(cwd: &Path, path: &str) -> bool {
    Command::new("git")
        .args(["check-ignore", "-q", path])
        .current_dir(cwd)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Get git user name.
pub fn get_git_user_name(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "user.name"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get git user email.
pub fn get_git_user_email(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "user.email"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Parse a git commit SHA from output text.
pub fn parse_commit_sha(text: &str) -> Option<String> {
    // Look for 7-40 hex character sequences that look like commit SHAs
    for word in text.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| !c.is_ascii_hexdigit());
        if cleaned.len() >= 7
            && cleaned.len() <= 40
            && cleaned.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Some(cleaned.to_string());
        }
    }
    None
}

/// Get the list of staged files.
pub fn get_staged_files(cwd: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Get the list of modified (unstaged) files.
pub fn get_modified_files(cwd: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Get untracked files.
pub fn get_untracked_files(cwd: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Generate a git diff for a specific file.
pub fn get_file_diff(cwd: &Path, file_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["diff", "--", file_path])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.is_empty() { None } else { Some(diff) }
    } else {
        None
    }
}

#[cfg(test)]
#[path = "git_operations.test.rs"]
mod tests;
