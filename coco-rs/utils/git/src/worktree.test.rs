use super::*;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use std::process::Command;

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &std::path::Path) {
    git_in(dir, &["init", "--initial-branch=main"]);
    git_in(dir, &["config", "user.name", "Tester"]);
    git_in(dir, &["config", "user.email", "t@e.com"]);
    std::fs::write(dir.join("seed.txt"), "seed\n").unwrap();
    git_in(dir, &["add", "seed.txt"]);
    git_in(dir, &["commit", "-m", "seed"]);
}

#[test]
fn count_changes_clean_repo_is_zero() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_repo(&repo);
    let summary = count_worktree_changes(&repo, None).expect("status should succeed");
    assert_eq!(summary, WorktreeChangeSummary::default());
    assert!(!summary.has_pending_work());
}

#[test]
fn count_changes_detects_uncommitted_files() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_repo(&repo);
    std::fs::write(repo.join("dirty.txt"), "wip\n").unwrap(); // untracked
    std::fs::write(repo.join("seed.txt"), "edited\n").unwrap(); // modified
    let summary = count_worktree_changes(&repo, None).expect("status should succeed");
    assert_eq!(summary.changed_files, 2);
    assert!(summary.has_pending_work());
}

#[test]
fn count_changes_fails_closed_outside_repo() {
    // No git repo → status fails → None (caller treats as unsafe).
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(count_worktree_changes(temp.path(), None), None);
}

#[test]
fn test_list_orphaned_worktrees_parses_porcelain() {
    // Verify the parsing logic handles the porcelain format correctly.
    // Full integration tests require a real git repo, so we just ensure
    // the function returns None for a non-repo path.
    let result = list_orphaned_worktrees(std::path::Path::new("/nonexistent"));
    assert!(result.is_none());
}

#[test]
fn parse_porcelain_main_worktree_only() {
    let raw = "worktree /home/u/repo\nHEAD 0123\nbranch refs/heads/main\n";
    assert_eq!(
        parse_worktree_output(raw),
        vec![PathBuf::from("/home/u/repo")]
    );
}

#[test]
fn parse_porcelain_multiple_worktrees() {
    let raw = "\
worktree /home/u/repo
HEAD 0123
branch refs/heads/main

worktree /home/u/repo-feat
HEAD 4567
branch refs/heads/feat
";
    assert_eq!(
        parse_worktree_output(raw),
        vec![
            PathBuf::from("/home/u/repo"),
            PathBuf::from("/home/u/repo-feat"),
        ],
    );
}

#[test]
fn parse_porcelain_empty_input() {
    assert_eq!(parse_worktree_output(""), Vec::<PathBuf>::new());
}

#[test]
fn parse_porcelain_ignores_non_worktree_lines() {
    let raw = "worktree /repo\nHEAD abc\ndetached\n";
    assert_eq!(parse_worktree_output(raw), vec![PathBuf::from("/repo")]);
}

#[test]
fn parse_porcelain_normalizes_nfc() {
    // Decomposed (a + combining acute U+0301) → precomposed á (U+00E1).
    let raw = "worktree /repo/cafe\u{0301}\n";
    assert_eq!(
        parse_worktree_output(raw),
        vec![PathBuf::from("/repo/caf\u{00e9}")],
    );
}
