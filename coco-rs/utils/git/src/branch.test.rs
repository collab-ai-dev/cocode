use super::merge_base_with_head;
use crate::GitToolingError;
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

fn run_git_stdout(repo_path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .expect("git command");
    assert!(output.status.success(), "git command failed: {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_test_repo(repo_path: &Path) {
    run_git_in(repo_path, &["init", "--initial-branch=main"]);
    run_git_in(repo_path, &["config", "core.autocrlf", "false"]);
}

fn commit(repo_path: &Path, message: &str) {
    run_git_in(
        repo_path,
        &[
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            message,
        ],
    );
}

#[test]
fn merge_base_returns_shared_commit() -> Result<(), GitToolingError> {
    let temp = tempdir()?;
    let repo = temp.path();
    init_test_repo(repo);

    std::fs::write(repo.join("base.txt"), "base\n")?;
    run_git_in(repo, &["add", "base.txt"]);
    commit(repo, "base commit");

    run_git_in(repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("feature.txt"), "feature change\n")?;
    run_git_in(repo, &["add", "feature.txt"]);
    commit(repo, "feature commit");

    run_git_in(repo, &["checkout", "main"]);
    std::fs::write(repo.join("main.txt"), "main change\n")?;
    run_git_in(repo, &["add", "main.txt"]);
    commit(repo, "main commit");

    run_git_in(repo, &["checkout", "feature"]);

    let expected = run_git_stdout(repo, &["merge-base", "HEAD", "main"]);
    let merge_base = merge_base_with_head(repo, "main")?;
    assert_eq!(merge_base, Some(expected));

    Ok(())
}

#[test]
fn merge_base_prefers_upstream_when_remote_ahead() -> Result<(), GitToolingError> {
    let temp = tempdir()?;
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir_all(&repo)?;
    std::fs::create_dir_all(&remote)?;

    run_git_in(&remote, &["init", "--bare"]);
    run_git_in(&repo, &["init", "--initial-branch=main"]);
    run_git_in(&repo, &["config", "core.autocrlf", "false"]);

    std::fs::write(repo.join("base.txt"), "base\n")?;
    run_git_in(&repo, &["add", "base.txt"]);
    commit(&repo, "base commit");

    run_git_in(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git_in(&repo, &["push", "-u", "origin", "main"]);

    run_git_in(&repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("feature.txt"), "feature change\n")?;
    run_git_in(&repo, &["add", "feature.txt"]);
    commit(&repo, "feature commit");

    run_git_in(&repo, &["checkout", "--orphan", "rewrite"]);
    run_git_in(&repo, &["rm", "-rf", "."]);
    std::fs::write(repo.join("new-main.txt"), "rewritten main\n")?;
    run_git_in(&repo, &["add", "new-main.txt"]);
    commit(&repo, "rewrite main");
    run_git_in(&repo, &["branch", "-M", "rewrite", "main"]);
    run_git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);

    run_git_in(&repo, &["checkout", "feature"]);
    run_git_in(&repo, &["fetch", "origin"]);

    let expected = run_git_stdout(&repo, &["merge-base", "HEAD", "origin/main"]);
    let merge_base = merge_base_with_head(&repo, "main")?;
    assert_eq!(merge_base, Some(expected));

    Ok(())
}

#[test]
fn merge_base_returns_none_when_branch_missing() -> Result<(), GitToolingError> {
    let temp = tempdir()?;
    let repo = temp.path();
    init_test_repo(repo);

    std::fs::write(repo.join("tracked.txt"), "tracked\n")?;
    run_git_in(repo, &["add", "tracked.txt"]);
    commit(repo, "initial");

    let merge_base = merge_base_with_head(repo, "missing-branch")?;
    assert_eq!(merge_base, None);

    Ok(())
}
