// Reads the process cwd, legitimate outside session-owned code; opts out of
// the workspace-wide `std::env::current_dir` gate (clippy.toml, §6.5/D-37).
#![allow(clippy::disallowed_methods)]

use super::*;

#[test]
fn test_get_main_branch() {
    // In our repo, main branch should exist
    let cwd = std::env::current_dir().unwrap();
    let branch = get_main_branch(&cwd);
    assert!(!branch.is_empty());
}

#[test]
fn test_get_git_root() {
    let cwd = std::env::current_dir().unwrap();
    let root = get_git_root(&cwd);
    // We're in a git repo
    assert!(root.is_some());
}
