use super::*;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .output()
        .expect("spawn ok");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // git init and minimal identity
    let _ = run(root, &["git", "init"]);
    let _ = run(root, &["git", "config", "user.email", "codex@example.com"]);
    let _ = run(root, &["git", "config", "user.name", "Codex"]);
    dir
}

fn read_file_normalized(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read file")
        .replace("\r\n", "\n")
}

#[test]
fn extract_paths_handles_quoted_headers() {
    let diff = "diff --git \"a/hello world.txt\" \"b/hello world.txt\"\nnew file mode 100644\n--- /dev/null\n+++ b/hello world.txt\n@@ -0,0 +1 @@\n+hi\n";
    let paths = extract_paths_from_patch(diff);
    assert_eq!(paths, vec!["hello world.txt".to_string()]);
}

#[test]
fn extract_paths_ignores_dev_null_header() {
    let diff = "diff --git a/dev/null b/ok.txt\nnew file mode 100644\n--- /dev/null\n+++ b/ok.txt\n@@ -0,0 +1 @@\n+hi\n";
    let paths = extract_paths_from_patch(diff);
    assert_eq!(paths, vec!["ok.txt".to_string()]);
}

#[test]
fn extract_paths_unescapes_c_style_in_quoted_headers() {
    let diff = "diff --git \"a/hello\\tworld.txt\" \"b/hello\\tworld.txt\"\nnew file mode 100644\n--- /dev/null\n+++ b/hello\tworld.txt\n@@ -0,0 +1 @@\n+hi\n";
    let paths = extract_paths_from_patch(diff);
    assert_eq!(paths, vec!["hello\tworld.txt".to_string()]);
}

#[test]
fn parse_output_unescapes_quoted_paths() {
    let stderr = "error: patch failed: \"hello\\tworld.txt\":1\n";
    let (applied, skipped, conflicted) = parse_git_apply_output("", stderr);
    assert_eq!(applied, Vec::<String>::new());
    assert_eq!(conflicted, Vec::<String>::new());
    assert_eq!(skipped, vec!["hello\tworld.txt".to_string()]);
}

#[test]
fn apply_add_success() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();

    let diff = "diff --git a/hello.txt b/hello.txt\nnew file mode 100644\n--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";
    let req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let r = apply_git_patch(&req).expect("run apply");
    assert_eq!(r.exit_code, 0, "exit code 0");
    // File exists now
    assert!(root.join("hello.txt").exists());
}

#[test]
fn apply_modify_conflict() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();
    // seed file and commit
    std::fs::write(root.join("file.txt"), "line1\nline2\nline3\n").unwrap();
    let _ = run(root, &["git", "add", "file.txt"]);
    let _ = run(root, &["git", "commit", "-m", "seed"]);
    // local edit (unstaged)
    std::fs::write(root.join("file.txt"), "line1\nlocal2\nline3\n").unwrap();
    // patch wants to change the same line differently
    let diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+remote2\n line3\n";
    let req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let r = apply_git_patch(&req).expect("run apply");
    assert_ne!(r.exit_code, 0, "non-zero exit on conflict");
}

#[test]
fn apply_modify_skipped_missing_index() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();
    // Try to modify a file that is not in the index
    let diff = "diff --git a/ghost.txt b/ghost.txt\n--- a/ghost.txt\n+++ b/ghost.txt\n@@ -1,1 +1,1 @@\n-old\n+new\n";
    let req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let r = apply_git_patch(&req).expect("run apply");
    assert_ne!(r.exit_code, 0, "non-zero exit on missing index");
}

#[test]
fn apply_then_revert_success() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();
    // Seed file and commit original content
    std::fs::write(root.join("file.txt"), "orig\n").unwrap();
    let _ = run(root, &["git", "add", "file.txt"]);
    let _ = run(root, &["git", "commit", "-m", "seed"]);

    // Forward patch: orig -> ORIG
    let diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n-orig\n+ORIG\n";
    let apply_req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let res_apply = apply_git_patch(&apply_req).expect("apply ok");
    assert_eq!(res_apply.exit_code, 0, "forward apply succeeded");
    let after_apply = read_file_normalized(&root.join("file.txt"));
    assert_eq!(after_apply, "ORIG\n");

    // Revert patch: ORIG -> orig (stage paths first; engine handles it)
    let revert_req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: true,
        preflight: false,
    };
    let res_revert = apply_git_patch(&revert_req).expect("revert ok");
    assert_eq!(res_revert.exit_code, 0, "revert apply succeeded");
    let after_revert = read_file_normalized(&root.join("file.txt"));
    assert_eq!(after_revert, "orig\n");
}

#[test]
fn revert_preflight_does_not_stage_index() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();
    // Seed repo and apply forward patch so the working tree reflects the change.
    std::fs::write(root.join("file.txt"), "orig\n").unwrap();
    let _ = run(root, &["git", "add", "file.txt"]);
    let _ = run(root, &["git", "commit", "-m", "seed"]);

    let diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n-orig\n+ORIG\n";
    let apply_req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let res_apply = apply_git_patch(&apply_req).expect("apply ok");
    assert_eq!(res_apply.exit_code, 0, "forward apply succeeded");
    let (commit_code, _, commit_err) = run(root, &["git", "commit", "-am", "apply change"]);
    assert_eq!(commit_code, 0, "commit applied change: {commit_err}");

    let (_code_before, staged_before, _stderr_before) =
        run(root, &["git", "diff", "--cached", "--name-only"]);

    let preflight_req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: true,
        preflight: true,
    };
    let res_preflight = apply_git_patch(&preflight_req).expect("preflight ok");
    assert_eq!(res_preflight.exit_code, 0, "revert preflight succeeded");
    let (_code_after, staged_after, _stderr_after) =
        run(root, &["git", "diff", "--cached", "--name-only"]);
    assert_eq!(
        staged_after.trim(),
        staged_before.trim(),
        "preflight should not stage new paths",
    );

    let after_preflight = read_file_normalized(&root.join("file.txt"));
    assert_eq!(after_preflight, "ORIG\n");
}

#[test]
fn preflight_blocks_partial_changes() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();
    // Build a multi-file diff: one valid add (ok.txt) and one invalid modify (ghost.txt)
    let diff = "diff --git a/ok.txt b/ok.txt\nnew file mode 100644\n--- /dev/null\n+++ b/ok.txt\n@@ -0,0 +1,2 @@\n+alpha\n+beta\n\n\
diff --git a/ghost.txt b/ghost.txt\n--- a/ghost.txt\n+++ b/ghost.txt\n@@ -1,1 +1,1 @@\n-old\n+new\n";

    // 1) With preflight enabled, nothing should be changed (even though ok.txt could be added)
    let req1 = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: true,
    };
    let r1 = apply_git_patch(&req1).expect("preflight apply");
    assert_ne!(r1.exit_code, 0, "preflight reports failure");
    assert!(
        !root.join("ok.txt").exists(),
        "preflight must prevent adding ok.txt"
    );
    assert!(
        r1.cmd_for_log.contains("--check"),
        "preflight path recorded --check"
    );

    // 2) Without preflight, we should see no --check in the executed command
    let req2 = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let r2 = apply_git_patch(&req2).expect("direct apply");
    assert_ne!(r2.exit_code, 0, "apply is expected to fail overall");
    assert!(
        !r2.cmd_for_log.contains("--check"),
        "non-preflight path should not use --check"
    );
}
