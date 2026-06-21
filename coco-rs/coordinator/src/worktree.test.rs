//! Tests for [`AgentWorktreeManager`].
//!
//! Uses real `git` subprocesses on a tempdir to exercise the full
//! create → inspect → cleanup lifecycle. Tests skip gracefully when
//! `git` isn't available (rare in CI).

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::*;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_repo(dir: &Path) {
    Command::new("git").arg("init").arg(dir).output().unwrap();
    // Minimal identity so commits work.
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.email", "test@coco.dev"])
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.name", "coco test"])
        .output()
        .unwrap();
    // Initial commit — worktree add needs at least one commit on HEAD.
    std::fs::write(dir.join("README.md"), "seed\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-m", "seed"])
        .output()
        .unwrap();
}

#[test]
fn test_validate_slug_rejects_bad_characters() {
    use super::validate_slug;
    assert!(validate_slug("agent-abc123").is_ok());
    assert!(validate_slug("").is_err());
    assert!(validate_slug("has/slash").is_err());
    assert!(validate_slug("has space").is_err());
    assert!(validate_slug("has;semi").is_err());
}

#[test]
fn test_discover_from_cwd_on_non_repo_returns_not_in_repo() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let result = AgentWorktreeManager::discover_from_cwd(tmp.path());
    assert!(matches!(result, Err(WorktreeError::NotInRepo { .. })));
}

#[test]
fn test_create_for_builds_worktree_under_canonical_root() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).expect("discover repo root");
    let session = manager
        .create_for("agent-abc12345")
        .expect("create worktree");

    assert!(session.path.exists());
    assert!(session.path.starts_with(manager.canonical_git_root()));
    assert!(
        session
            .path
            .ends_with(Path::new(".coco/worktrees/agent-abc12345"))
    );
    assert!(!session.head_commit.is_empty());
    assert_eq!(session.branch, "claude/agent-abc12345");
}

#[test]
fn test_create_for_rejects_invalid_slug() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let err = manager.create_for("../evil").unwrap_err();
    assert!(matches!(err, WorktreeError::InvalidSlug { .. }));
}

#[test]
fn test_cleanup_removes_worktree_when_no_changes() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-nochange").unwrap();
    let path = session.path.clone();

    let outcome = manager.cleanup_if_unchanged(session);
    assert!(matches!(outcome, WorktreeCleanupOutcome::Removed));
    assert!(!path.exists(), "worktree dir should be gone");
}

#[test]
fn test_cleanup_keeps_worktree_when_files_changed() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-haschange").unwrap();

    // Simulate the child agent creating a file in the worktree.
    std::fs::write(session.path.join("new_file.txt"), "hi").unwrap();

    let path = session.path.clone();
    let branch = session.branch.clone();
    let outcome = manager.cleanup_if_unchanged(session);
    match outcome {
        WorktreeCleanupOutcome::Kept {
            path: p,
            branch: b,
            reason,
        } => {
            assert_eq!(p, path);
            assert_eq!(b, branch);
            assert_eq!(reason, KeptReason::HasChanges);
        }
        _ => panic!("expected Kept on changed worktree, got {outcome:?}"),
    }
    assert!(path.exists(), "worktree with changes must survive cleanup");
}

fn commit_in(dir: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-m", message])
        .output()
        .unwrap();
}

#[test]
fn test_cleanup_keeps_worktree_when_commits_made() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-commit01").unwrap();

    // The child agent COMMITS its output: the working tree ends clean but HEAD
    // has advanced past the commit the worktree was created from. Without the
    // commit-since-creation check this looked like "no changes" and the
    // worktree + branch were force-removed, destroying the commit.
    commit_in(&session.path, "work.txt", "agent output", "agent work");

    let path = session.path.clone();
    let outcome = manager.cleanup_if_unchanged(session);
    assert!(
        matches!(
            outcome,
            WorktreeCleanupOutcome::Kept {
                reason: KeptReason::HasChanges,
                ..
            }
        ),
        "worktree with a new commit must be kept, got {outcome:?}"
    );
    assert!(path.exists(), "committed agent work must survive cleanup");
}

#[test]
fn test_cleanup_stale_keeps_worktree_with_unpushed_commits() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-abcdef01").unwrap();

    // Commit work, leaving a clean tree. With no remote configured, the commit
    // is "unpushed" — the stale sweep must preserve it even though it's old and
    // its working tree is clean.
    commit_in(&session.path, "wip.txt", "x", "wip");

    let removed = manager.cleanup_stale(std::time::Duration::from_secs(0));
    assert_eq!(removed, 0, "unpushed commits must block the stale sweep");
    assert!(
        session.path.exists(),
        "worktree with unpushed commits must survive"
    );
}

#[test]
fn test_post_creation_setup_copies_settings_local() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    // Create .coco/settings.local.json in the main repo.
    let settings_dir = tmp.path().join(".coco");
    std::fs::create_dir_all(&settings_dir).unwrap();
    std::fs::write(
        settings_dir.join("settings.local.json"),
        r#"{"some":"setting"}"#,
    )
    .unwrap();

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-settings").unwrap();

    let copied = session.path.join(".coco").join("settings.local.json");
    assert!(
        copied.exists(),
        "post-creation setup should have copied settings.local.json"
    );
    let content = std::fs::read_to_string(&copied).unwrap();
    assert!(content.contains("some"));
}

#[test]
fn test_is_agent_slug_narrow_shape() {
    use super::is_agent_slug;
    assert!(is_agent_slug("agent-abcd1234"));
    assert!(is_agent_slug("agent-00000000"));
    assert!(!is_agent_slug("agent-xyz12345"), "non-hex must reject");
    assert!(
        !is_agent_slug("agent-abcd12345"),
        "9 chars must reject — must be exactly 8"
    );
    assert!(!is_agent_slug("agent-abcd123"), "7 chars must reject");
    assert!(!is_agent_slug("wt-myfeature"), "user slugs must not match");
    assert!(!is_agent_slug("agent-"), "empty suffix must reject");
    assert!(!is_agent_slug("random-dir"), "non-agent prefix must reject");
}

/// Push `repo`'s current HEAD to a fresh bare remote so its commits are
/// remote-reachable — required for the stale sweep to consider a clean,
/// commit-free agent worktree removable (the unpushed-commit guard otherwise
/// keeps everything in a remote-less repo).
fn add_remote_and_push(repo: &Path, remote: &Path) {
    Command::new("git")
        .args(["init", "--bare"])
        .arg(remote)
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "add", "origin"])
        .arg(remote)
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["push", "origin", "HEAD"])
        .output()
        .unwrap();
    // Populate refs/remotes/origin/* so `rev-list HEAD --not --remotes` sees
    // the pushed commits even on git versions that don't auto-create the
    // tracking ref on push.
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fetch", "origin"])
        .output()
        .unwrap();
}

#[test]
fn test_cleanup_stale_removes_old_agent_worktree_without_changes() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let remote = TempDir::new().unwrap();
    init_repo(tmp.path());
    // The base commits must be on a remote, else the unpushed-commit guard
    // (fail-closed) keeps the worktree.
    add_remote_and_push(tmp.path(), remote.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-abcd1234").unwrap();
    let path = session.path.clone();
    // Don't run cleanup_if_unchanged — simulate a crashed session.
    drop(session);

    // Tiny wait so the mtime recorded by `create_for` is reliably
    // in the past relative to the sweep's `SystemTime::now()`.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Threshold of 1ms: any worktree older than 1ms is a sweep
    // candidate. Clean tree + all commits pushed → removable.
    let removed = manager.cleanup_stale(std::time::Duration::from_millis(1));
    assert_eq!(removed, 1);
    assert!(!path.exists(), "stale worktree dir should be gone");
}

#[test]
fn test_cleanup_stale_ignores_recent_worktrees() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-fedcba98").unwrap();
    let path = session.path.clone();
    drop(session);

    // No back-date: mtime is now. Sweep with a 30-day threshold
    // must NOT touch it.
    let removed = manager.cleanup_stale(std::time::Duration::from_secs(30 * 86400));
    assert_eq!(removed, 0);
    assert!(path.exists(), "recent worktree must survive sweep");
}

#[test]
fn test_cleanup_stale_preserves_worktrees_with_changes() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let session = manager.create_for("agent-11223344").unwrap();
    let path = session.path.clone();
    // Simulate uncommitted work inside the worktree.
    std::fs::write(path.join("work.txt"), "important").unwrap();
    drop(session);

    std::thread::sleep(std::time::Duration::from_millis(50));
    let removed = manager.cleanup_stale(std::time::Duration::from_millis(1));
    assert_eq!(removed, 0, "sweep must preserve worktrees with changes");
    assert!(path.exists(), "user's work must survive stale sweep");
}

#[test]
fn test_cleanup_stale_skips_user_named_worktrees() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    // Create a user-named worktree manually (not agent slug pattern).
    let user_wt = tmp
        .path()
        .join(".coco")
        .join("worktrees")
        .join("wt-myfeature");
    std::fs::create_dir_all(user_wt.parent().unwrap()).unwrap();
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["worktree", "add", "-B", "user-branch"])
        .arg(&user_wt)
        .output()
        .unwrap();
    if !output.status.success() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(50));

    let manager = AgentWorktreeManager::discover_from_cwd(tmp.path()).unwrap();
    let removed = manager.cleanup_stale(std::time::Duration::from_millis(1));
    assert_eq!(removed, 0, "user-named worktrees must never be swept");
    assert!(user_wt.exists());
}

#[test]
fn test_symlink_directories_mirrors_configured_dirs_into_worktree() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    // Create a "node_modules"-like dir in the main repo.
    let node_modules = tmp.path().join("node_modules");
    std::fs::create_dir_all(&node_modules).unwrap();
    std::fs::write(node_modules.join("package.json"), "{}").unwrap();

    let manager = AgentWorktreeManager::new(tmp.path().canonicalize().unwrap()).with_config(
        AgentWorktreeConfig {
            symlink_directories: vec![PathBuf::from("node_modules")],
        },
    );
    let session = manager.create_for("agent-cafe1234").unwrap();

    let linked = session.path.join("node_modules");
    assert!(
        linked.exists(),
        "symlinked dir should be visible in worktree"
    );
    // Confirm it's actually a symlink (not a copy).
    let metadata = std::fs::symlink_metadata(&linked).unwrap();
    assert!(
        metadata.file_type().is_symlink(),
        "node_modules inside worktree must be a symlink, not a copy"
    );
    // Confirm the symlink target content is reachable.
    let pkg = linked.join("package.json");
    assert!(pkg.exists());
}

#[test]
fn test_symlink_directories_skips_missing_sources() {
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    // Don't create "node_modules" — symlink config references a
    // missing dir. Must silently skip, not fail creation.
    let manager = AgentWorktreeManager::new(tmp.path().canonicalize().unwrap()).with_config(
        AgentWorktreeConfig {
            symlink_directories: vec![PathBuf::from("node_modules")],
        },
    );
    let session = manager.create_for("agent-deadbeef").unwrap();
    assert!(session.path.exists());
    assert!(!session.path.join("node_modules").exists());
}

#[test]
fn test_has_worktree_create_hook_returns_false_for_empty_registry() {
    let registry = coco_hooks::HookRegistry::new();
    assert!(!AgentWorktreeManager::has_worktree_create_hook(&registry));
}

#[test]
fn test_has_worktree_create_hook_returns_true_when_hook_registered() {
    use coco_types::HookEventType;
    use coco_types::HookScope;
    let registry = coco_hooks::HookRegistry::new();
    registry.register(coco_hooks::HookDefinition {
        event: HookEventType::WorktreeCreate,
        matcher: None,
        handler: coco_hooks::HookHandler::Command {
            command: "true".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    assert!(AgentWorktreeManager::has_worktree_create_hook(&registry));
}

#[test]
fn test_discover_canonical_root_from_inside_worktree() {
    // Ensures nested spawns (from within a worktree) still resolve
    // to the main repo, not the worktree itself.
    if !git_available() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());

    // Create a worktree manually.
    let wt = tmp.path().join("nested-wt");
    let output = Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["worktree", "add", "-B", "test-branch"])
        .arg(&wt)
        .output()
        .unwrap();
    if !output.status.success() {
        eprintln!("git worktree add failed: {output:?}");
        return;
    }

    // Discover from inside the worktree — should return the main
    // repo's root, not the worktree path.
    let manager = AgentWorktreeManager::discover_from_cwd(&wt).expect("discover");
    let tmp_canonical = tmp.path().canonicalize().unwrap();
    assert_eq!(
        manager.canonical_git_root().canonicalize().unwrap(),
        tmp_canonical,
        "canonical root must be the main repo, not the nested worktree"
    );
}
