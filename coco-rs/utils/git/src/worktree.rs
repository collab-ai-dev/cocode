use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use unicode_normalization::UnicodeNormalization;

/// Branch prefix for agent worktrees.
const AGENT_WORKTREE_BRANCH_PREFIX: &str = "agent/task-";

/// Wall-clock cap on the `git worktree list` subprocess — matches TS
/// `{timeout: 5000}` in `getWorktreePathsPortable.ts`. A hung git
/// (corrupt repo, paused parent, network FS stall) must not block
/// session bootstrap, so we kill and return an empty Vec on timeout.
const WORKTREE_LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Return the absolute paths of every worktree associated with the
/// repo containing `cwd`, NFC-normalised. Order is whatever
/// `git worktree list --porcelain` produces (typically the main
/// worktree first, then linked worktrees in creation order).
///
/// Empty Vec on any error (binary missing, not a repo, timeout, …).
/// Callers treat "no worktrees" as "fallback inactive" — TS parity
/// with `sessionStoragePortable.ts:430`.
pub fn worktree_paths(cwd: &Path) -> Vec<PathBuf> {
    run_worktree_list(cwd).unwrap_or_default()
}

fn run_worktree_list(cwd: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut child = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;

    let deadline = Instant::now() + WORKTREE_LIST_TIMEOUT;
    loop {
        match child.try_wait()? {
            Some(status) => {
                if !status.success() {
                    return Ok(Vec::new());
                }
                let output = child.wait_with_output()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Ok(parse_worktree_output(&stdout));
            }
            None => {
                if Instant::now() >= deadline {
                    // Best-effort kill; if it fails the child becomes
                    // a zombie until the OS reaps it, but we still
                    // return cleanly so session resolution doesn't
                    // block.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Vec::new());
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Pure-function parser exposed for testing; consumes the stdout of
/// `git worktree list --porcelain` and returns the worktree paths
/// (NFC-normalized for filesystem-stable comparison).
pub fn parse_worktree_output(stdout: &str) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|p| PathBuf::from(p.nfc().collect::<String>()))
        .collect()
}

/// Pending-work summary for a worktree, used as a removal safety gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorktreeChangeSummary {
    /// Uncommitted (staged + unstaged + untracked) files.
    pub changed_files: usize,
    /// Commits on `HEAD` not reachable from `base_commit`, when one is given.
    pub commits_ahead: usize,
}

impl WorktreeChangeSummary {
    /// True when the worktree has work that removal would discard.
    pub fn has_pending_work(&self) -> bool {
        self.changed_files > 0 || self.commits_ahead > 0
    }
}

/// Count uncommitted files (and, when `base_commit` is given, commits ahead of
/// it) in `worktree_path`.
///
/// Returns `None` when git state cannot be reliably determined (the
/// `status` / `rev-list` subprocess failed — lock file, corrupt index, bad
/// ref). Callers MUST treat `None` as "unknown, assume unsafe" (fail-closed),
/// mirroring TS `countWorktreeChanges` — a silent `0/0` would let a forced
/// removal destroy real work. `base_commit` is `None` for callers that have no
/// baseline; the uncommitted-file count is still authoritative.
pub fn count_worktree_changes(
    worktree_path: &Path,
    base_commit: Option<&str>,
) -> Option<WorktreeChangeSummary> {
    let status =
        crate::operations::run_git_for_stdout(worktree_path, ["status", "--porcelain"], None)
            .ok()?;
    let changed_files = status.lines().filter(|l| !l.trim().is_empty()).count();

    let commits_ahead = match base_commit {
        Some(base) => {
            let range = format!("{base}..HEAD");
            let out = crate::operations::run_git_for_stdout(
                worktree_path,
                ["rev-list", "--count", range.as_str()],
                None,
            )
            .ok()?;
            out.trim().parse::<usize>().ok()?
        }
        None => 0,
    };

    Some(WorktreeChangeSummary {
        changed_files,
        commits_ahead,
    })
}

/// An orphaned worktree discovered during cleanup.
#[derive(Debug)]
struct OrphanedWorktree {
    path: String,
    branch: String,
}

/// Clean up orphaned agent worktrees and their branches.
///
/// Scans for worktrees whose branches match the `agent/task-*` naming
/// convention (auto-generated by `EnterWorktree`), removes them, deletes
/// the associated branches, and prunes stale bookkeeping entries.
///
/// Returns the number of worktrees cleaned up.
pub fn cleanup_orphaned_worktrees(cwd: &Path) -> i32 {
    if !crate::is_inside_git_repo(cwd) {
        return 0;
    }

    let orphans = match list_orphaned_worktrees(cwd) {
        Some(v) => v,
        None => return 0,
    };

    let count = orphans.len() as i32;
    for orphan in &orphans {
        // Remove the worktree directory
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["worktree", "remove", "--force", &orphan.path])
            .output();

        // Delete the orphaned branch
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["branch", "-D", &orphan.branch])
            .output();
    }

    let _ = Command::new("git")
        .current_dir(cwd)
        .args(["worktree", "prune"])
        .output();

    count
}

/// Parse `git worktree list --porcelain` to find agent/task-* worktrees.
fn list_orphaned_worktrees(cwd: &Path) -> Option<Vec<OrphanedWorktree>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut orphans = Vec::new();
    let mut worktree_path: Option<String> = None;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            worktree_path = Some(path.to_string());
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if branch.starts_with(AGENT_WORKTREE_BRANCH_PREFIX)
                && let Some(wt_path) = worktree_path.take()
            {
                orphans.push(OrphanedWorktree {
                    path: wt_path,
                    branch: branch.to_string(),
                });
            }
            worktree_path = None;
        } else if line.is_empty() {
            worktree_path = None;
        }
    }

    Some(orphans)
}

#[cfg(test)]
#[path = "worktree.test.rs"]
mod tests;
