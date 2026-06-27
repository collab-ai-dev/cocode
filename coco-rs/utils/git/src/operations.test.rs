use super::find_canonical_git_root;
use super::get_default_branch;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn run_git_in(repo_path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git command failed: {args:?}");
}

fn init_test_repo(repo_path: &Path) {
    run_git_in(repo_path, &["init", "--initial-branch=main"]);
    run_git_in(repo_path, &["config", "core.autocrlf", "false"]);
    run_git_in(repo_path, &["config", "user.name", "Tester"]);
    run_git_in(repo_path, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo_path.join("seed.txt"), "seed\n").unwrap();
    run_git_in(repo_path, &["add", "seed.txt"]);
    run_git_in(repo_path, &["commit", "-m", "seed"]);
}

#[test]
fn returns_none_outside_repo() {
    let temp = tempdir().unwrap();
    assert_eq!(find_canonical_git_root(temp.path()), None);
}

#[test]
fn returns_repo_root_for_main_worktree() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);

    let root = find_canonical_git_root(&repo).expect("should resolve");
    assert_eq!(root.canonicalize().unwrap(), repo);
}

#[test]
fn returns_same_root_from_subdirectory() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);
    let sub = repo.join("nested/dir");
    std::fs::create_dir_all(&sub).unwrap();

    let root = find_canonical_git_root(&sub).expect("should resolve");
    assert_eq!(root.canonicalize().unwrap(), repo);
}

#[test]
fn linked_worktrees_share_canonical_root() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);

    let wt_path = temp.path().join("wt-feature");
    run_git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ],
    );

    let main_root = find_canonical_git_root(&repo).expect("main root");
    let wt_root = find_canonical_git_root(&wt_path).expect("worktree root");
    // Both should canonicalize to the main repo path — that's the
    // shared identity worktrees agree on.
    assert_eq!(
        main_root.canonicalize().unwrap(),
        wt_root.canonicalize().unwrap(),
    );
}

#[test]
fn get_default_branch_falls_back_to_main_without_remote() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);
    // No origin remote/refs at all → hardcoded "main" fallback.
    assert_eq!(get_default_branch(&repo), "main");
}

#[test]
fn get_default_branch_reads_origin_head_symref() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);
    // Point a fake origin/master at HEAD and set origin/HEAD -> origin/master.
    run_git_in(&repo, &["update-ref", "refs/remotes/origin/master", "HEAD"]);
    run_git_in(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/master",
        ],
    );
    // Must READ the symref (master), not blindly default to "main".
    assert_eq!(get_default_branch(&repo), "master");
}

#[test]
fn get_default_branch_falls_back_to_local_origin_main() {
    let temp = tempdir().unwrap();
    let repo = temp.path().canonicalize().unwrap();
    init_test_repo(&repo);
    // origin/main exists locally but no origin/HEAD symref → resolves to it.
    run_git_in(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert_eq!(get_default_branch(&repo), "main");
}
