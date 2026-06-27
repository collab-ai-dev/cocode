use super::*;

#[test]
fn test_check_dangerous_path_root() {
    assert!(check_dangerous_path("rm", "/", "/home/user/project").is_some());
    assert!(check_dangerous_path("rm", "/etc", "/home/user/project").is_some());
    assert!(check_dangerous_path("rm", "/usr", "/home/user/project").is_some());
}

#[test]
fn test_check_dangerous_path_safe() {
    assert!(check_dangerous_path("rm", "file.txt", "/home/user/project").is_none());
    assert!(check_dangerous_path("rm", "/tmp/test", "/home/user/project").is_none());
}

#[test]
fn test_filter_flags() {
    assert_eq!(filter_flags(&["-la", "dir"]), vec!["dir"]);
    assert_eq!(filter_flags(&["--", "-file"]), vec!["-file"]);
}

#[test]
fn test_extract_find_paths() {
    assert_eq!(extract_find_paths(&[".", "-name", "*.rs"]), vec!["."]);
    assert_eq!(
        extract_find_paths(&["/src", "/lib", "-type", "f"]),
        vec!["/src", "/lib"]
    );
}

#[test]
fn test_extract_pattern_paths() {
    assert_eq!(
        extract_pattern_command_paths(&["pattern", "file1.rs", "file2.rs"]),
        vec!["file1.rs", "file2.rs"]
    );
}

#[test]
fn test_expand_home() {
    let expanded = expand_home("~/Documents");
    assert!(expanded.ends_with("/Documents"));
    assert!(!expanded.starts_with('~'));
}

// ── force-ask gates (P4/P15) ──

#[test]
fn test_check_dangerous_removal() {
    // Catastrophic removals → force-ask (even compounded / wrapped).
    assert!(check_dangerous_removal("rm -rf /", "/home/u/proj").is_some());
    assert!(check_dangerous_removal("rm -rf /etc", "/home/u/proj").is_some());
    assert!(check_dangerous_removal("ls && rm -rf /usr", "/home/u/proj").is_some());
    // Safe removals under cwd → no gate.
    assert!(check_dangerous_removal("rm -rf build", "/home/u/proj").is_none());
    assert!(check_dangerous_removal("rm foo.txt", "/home/u/proj").is_none());
    // Non-removal commands → no gate.
    assert!(check_dangerous_removal("ls /etc", "/home/u/proj").is_none());
}

#[test]
fn test_has_git_escape_pattern() {
    // cd + git compound → escape pattern.
    assert!(has_git_escape_pattern("cd /tmp/x && git status"));
    assert!(has_git_escape_pattern_in_cwd(
        "cd /tmp/other && git status",
        "/tmp/project"
    ));
    assert!(!has_git_escape_pattern_in_cwd(
        "cd /tmp/project && git status",
        "/tmp/project"
    ));
    // mkdir of a git-internal dir then git → escape.
    assert!(has_git_escape_pattern("mkdir refs && git init"));
    // Plain git / plain cd → not an escape.
    assert!(!has_git_escape_pattern("git status"));
    assert!(!has_git_escape_pattern("cd /tmp/x && ls"));
}

#[test]
fn test_check_multiple_cwd_changes() {
    assert!(check_multiple_cwd_changes("cd a && cd b && ls", "/tmp/project").is_some());
    assert!(check_multiple_cwd_changes("cd /tmp/project && cd . && ls", "/tmp/project").is_none());
}

#[test]
fn test_extract_write_path_targets() {
    // Write/create commands yield their path args.
    assert_eq!(
        extract_write_path_targets("cp a.txt /opt/b"),
        vec!["a.txt".to_string(), "/opt/b".to_string()]
    );
    assert_eq!(
        extract_write_path_targets("mkdir -p out/sub"),
        vec!["out/sub".to_string()]
    );
    assert_eq!(
        extract_write_path_targets("touch /tmp/x"),
        vec!["/tmp/x".to_string()]
    );
    // Compound: each write subcommand contributes.
    assert_eq!(
        extract_write_path_targets("rm foo && touch bar"),
        vec!["foo".to_string(), "bar".to_string()]
    );
    // Leading env vars / safe wrappers are stripped before classification.
    assert_eq!(
        extract_write_path_targets("FOO=1 timeout 5 rm out.txt"),
        vec!["out.txt".to_string()]
    );
}

#[test]
fn test_extract_write_path_targets_ignores_reads() {
    // Read / non-filesystem commands contribute no write targets.
    assert!(extract_write_path_targets("cat /etc/os-release").is_empty());
    assert!(extract_write_path_targets("ls -la /usr").is_empty());
    assert!(extract_write_path_targets("grep foo /etc/hosts").is_empty());
    assert!(extract_write_path_targets("echo hi").is_empty());
    assert!(extract_write_path_targets("git status").is_empty());
}

/// In-memory [`PathFileSystem`] for the bare-repo-escape tests: only the
/// explicitly-planted paths exist, with the declared kind.
#[derive(Default)]
struct MockPathFileSystem {
    files: std::collections::HashSet<std::path::PathBuf>,
    dirs: std::collections::HashSet<std::path::PathBuf>,
}

impl MockPathFileSystem {
    fn with_file(mut self, cwd: &str, rel: &str) -> Self {
        self.files.insert(std::path::Path::new(cwd).join(rel));
        self
    }
    fn with_dir(mut self, cwd: &str, rel: &str) -> Self {
        self.dirs.insert(std::path::Path::new(cwd).join(rel));
        self
    }
}

impl PathFileSystem for MockPathFileSystem {
    fn exists(&self, path: &std::path::Path) -> bool {
        self.files.contains(path) || self.dirs.contains(path)
    }
    fn is_file(&self, path: &std::path::Path) -> bool {
        self.files.contains(path)
    }
    fn is_dir(&self, path: &std::path::Path) -> bool {
        self.dirs.contains(path)
    }
}

const CWD: &str = "/work/planted";

#[test]
fn test_bare_repo_detected_on_full_internal_triad() {
    // HEAD file + objects/ + refs/ and no .git → planted bare repo.
    let fs = MockPathFileSystem::default()
        .with_file(CWD, "HEAD")
        .with_dir(CWD, "objects")
        .with_dir(CWD, "refs");
    assert!(is_current_dir_bare_git_repo_with_fs(CWD, &fs));
}

#[test]
fn test_bare_repo_ignored_when_dot_git_present() {
    // A normal working tree (has .git) is never treated as a bare repo, even
    // with the triad also present.
    let fs = MockPathFileSystem::default()
        .with_dir(CWD, ".git")
        .with_file(CWD, "HEAD")
        .with_dir(CWD, "objects")
        .with_dir(CWD, "refs");
    assert!(!is_current_dir_bare_git_repo_with_fs(CWD, &fs));
}

#[test]
fn test_bare_repo_requires_every_triad_member() {
    // Missing any one of HEAD / objects / refs → not a bare repo.
    let missing_head = MockPathFileSystem::default()
        .with_dir(CWD, "objects")
        .with_dir(CWD, "refs");
    assert!(!is_current_dir_bare_git_repo_with_fs(CWD, &missing_head));

    let missing_objects = MockPathFileSystem::default()
        .with_file(CWD, "HEAD")
        .with_dir(CWD, "refs");
    assert!(!is_current_dir_bare_git_repo_with_fs(CWD, &missing_objects));

    let missing_refs = MockPathFileSystem::default()
        .with_file(CWD, "HEAD")
        .with_dir(CWD, "objects");
    assert!(!is_current_dir_bare_git_repo_with_fs(CWD, &missing_refs));
}

#[test]
fn test_bare_repo_rejects_head_as_directory() {
    // `HEAD` must be a FILE; a directory named HEAD does not satisfy the triad.
    let fs = MockPathFileSystem::default()
        .with_dir(CWD, "HEAD")
        .with_dir(CWD, "objects")
        .with_dir(CWD, "refs");
    assert!(!is_current_dir_bare_git_repo_with_fs(CWD, &fs));
}
