use std::path::Path;
use std::path::PathBuf;

use super::*;

// ==========================================================================
// EnforcementLevel tests
// ==========================================================================

#[test]
fn test_enforcement_level_default() {
    assert_eq!(EnforcementLevel::default(), EnforcementLevel::Disabled);
}

#[test]
fn test_enforcement_level_serde_roundtrip() {
    for level in [
        EnforcementLevel::Disabled,
        EnforcementLevel::ReadOnly,
        EnforcementLevel::WorkspaceWrite,
        EnforcementLevel::Strict,
    ] {
        let json = serde_json::to_string(&level).expect("serialize");
        let parsed: EnforcementLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, level);
    }
}

#[test]
fn test_enforcement_level_kebab_case() {
    assert_eq!(
        serde_json::to_string(&EnforcementLevel::Disabled).expect("serialize"),
        "\"disabled\""
    );
    assert_eq!(
        serde_json::to_string(&EnforcementLevel::ReadOnly).expect("serialize"),
        "\"read-only\""
    );
    assert_eq!(
        serde_json::to_string(&EnforcementLevel::WorkspaceWrite).expect("serialize"),
        "\"workspace-write\""
    );
    assert_eq!(
        serde_json::to_string(&EnforcementLevel::Strict).expect("serialize"),
        "\"strict\""
    );
}

#[test]
fn test_enforcement_level_from_protocol() {
    assert_eq!(
        EnforcementLevel::from(SandboxMode::ReadOnly),
        EnforcementLevel::ReadOnly
    );
    assert_eq!(
        EnforcementLevel::from(SandboxMode::WorkspaceWrite),
        EnforcementLevel::WorkspaceWrite
    );
    assert_eq!(
        EnforcementLevel::from(SandboxMode::FullAccess),
        EnforcementLevel::Disabled
    );
    assert_eq!(
        EnforcementLevel::from(SandboxMode::ExternalSandbox),
        EnforcementLevel::WorkspaceWrite
    );
}

// ==========================================================================
// WritableRoot tests
// ==========================================================================

#[test]
fn test_writable_root_default_subpaths() {
    let root = WritableRoot::new("/home/user/project");
    assert_eq!(root.readonly_subpaths, vec![".git", ".coco", ".agents"]);
}

#[test]
fn test_writable_root_is_writable() {
    let root = WritableRoot::new("/home/user/project");
    // Normal files under root are writable
    assert!(root.is_writable(Path::new("/home/user/project/src/main.rs")));
    // .git subpath is read-only
    assert!(!root.is_writable(Path::new("/home/user/project/.git/config")));
    assert!(!root.is_writable(Path::new("/home/user/project/.git")));
    // .coco subpath is read-only
    assert!(!root.is_writable(Path::new("/home/user/project/.coco/config.json")));
    // .agents subpath is read-only
    assert!(!root.is_writable(Path::new("/home/user/project/.agents/skills")));
    // Paths outside root are not writable
    assert!(!root.is_writable(Path::new("/etc/passwd")));
}

#[test]
fn test_writable_root_resolved_readonly_subpaths() {
    let root = WritableRoot::new("/home/user/project");
    let resolved = root.resolved_readonly_subpaths();
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[0], Path::new("/home/user/project/.git"));
    assert_eq!(resolved[1], Path::new("/home/user/project/.coco"));
    assert_eq!(resolved[2], Path::new("/home/user/project/.agents"));
}

#[test]
fn test_writable_root_unprotected() {
    let root = WritableRoot::unprotected("/tmp/work");
    assert!(root.is_writable(Path::new("/tmp/work/.git/config")));
    assert!(root.is_writable(Path::new("/tmp/work/file.txt")));
}

#[test]
fn test_writable_root_contains() {
    let root = WritableRoot::new("/home/user/project");
    assert!(root.contains(Path::new("/home/user/project/src")));
    assert!(root.contains(Path::new("/home/user/project/.git")));
    assert!(!root.contains(Path::new("/home/user/other")));
}

#[test]
fn test_writable_root_serde_roundtrip() {
    let root = WritableRoot::new("/home/user/project");
    let json = serde_json::to_string(&root).expect("serialize");
    let parsed: WritableRoot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, root);
}

#[test]
fn test_writable_root_serde_default_subpaths() {
    // JSON without readonly_subpaths should use defaults
    let json = r#"{"path":"/tmp/work"}"#;
    let parsed: WritableRoot = serde_json::from_str(json).expect("parse");
    assert_eq!(parsed.readonly_subpaths, vec![".git", ".coco", ".agents"]);
}

// ==========================================================================
// SandboxConfig (runtime/adapter output) tests
// ==========================================================================

#[test]
fn test_sandbox_config_default() {
    let config = SandboxConfig::default();
    assert_eq!(config.enforcement, EnforcementLevel::Disabled);
    assert!(config.writable_roots.is_empty());
    assert!(config.denied_paths.is_empty());
    assert!(config.allowed_read_paths.is_empty());
    assert!(!config.allow_network);
}

#[test]
fn test_sandbox_config_serde_roundtrip() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::Strict,
        writable_roots: vec![WritableRoot::new("/home/user/project")],
        denied_paths: vec![PathBuf::from("/etc/passwd")],
        allowed_read_paths: vec![PathBuf::from("/etc/shadow/public")],
        allow_network: true,
        ..Default::default()
    };

    let json = serde_json::to_string(&config).expect("serialize");
    let parsed: SandboxConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.enforcement, EnforcementLevel::Strict);
    assert_eq!(parsed.writable_roots.len(), 1);
    assert_eq!(parsed.denied_paths.len(), 1);
    assert_eq!(parsed.allowed_read_paths.len(), 1);
    assert!(parsed.allow_network);
}

#[test]
fn test_sandbox_config_from_empty_json() {
    let config: SandboxConfig = serde_json::from_str("{}").expect("parse");
    assert_eq!(config.enforcement, EnforcementLevel::Disabled);
    assert!(config.writable_roots.is_empty());
    assert!(config.denied_paths.is_empty());
    assert!(config.allowed_read_paths.is_empty());
    assert!(!config.allow_network);
}

#[test]
fn test_sandbox_config_partial_json() {
    let config: SandboxConfig = serde_json::from_str(r#"{"enforcement":"strict"}"#).expect("parse");
    assert_eq!(config.enforcement, EnforcementLevel::Strict);
    assert!(config.writable_roots.is_empty());
    assert!(!config.allow_network);
}

// ==========================================================================
// Git pointer file detection
// ==========================================================================

#[test]
fn test_writable_root_detects_git_pointer_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let gitdir = root.join("actual_gitdir");
    std::fs::create_dir_all(&gitdir).expect("create gitdir");

    // Create a .git pointer file like git worktrees use
    std::fs::write(root.join(".git"), format!("gitdir: {}", gitdir.display())).expect("write");

    let wr = WritableRoot::new(root);
    // Should contain default subpaths plus the resolved gitdir
    assert!(wr.readonly_subpaths.contains(&".git".to_string()));
    assert!(wr.readonly_subpaths.contains(&".coco".to_string()));
    let gitdir_rel = gitdir
        .strip_prefix(root)
        .expect("strip")
        .display()
        .to_string();
    assert!(
        wr.readonly_subpaths.contains(&gitdir_rel),
        "Should contain resolved gitdir: {gitdir_rel}, got: {:?}",
        wr.readonly_subpaths
    );
}

#[test]
fn test_writable_root_git_dir_no_pointer_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Create a normal .git directory (not a pointer file)
    std::fs::create_dir_all(root.join(".git")).expect("create .git");

    let wr = WritableRoot::new(root);
    // Default subpaths only — no extra gitdir resolution
    assert_eq!(wr.readonly_subpaths, default_readonly_subpaths());
}

#[test]
fn test_writable_root_no_git_at_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wr = WritableRoot::new(dir.path());
    assert_eq!(wr.readonly_subpaths, default_readonly_subpaths());
}

#[test]
fn test_writable_root_git_pointer_relative_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let gitdir = root.join("..").join("shared_git");
    std::fs::create_dir_all(&gitdir).expect("create gitdir");

    // Create .git pointer with relative path
    std::fs::write(root.join(".git"), "gitdir: ../shared_git").expect("write");

    let wr = WritableRoot::new(root);
    // Relative gitdir outside root → should warn but not add (can't strip_prefix)
    // Just check it doesn't panic and has default subpaths
    assert!(wr.readonly_subpaths.contains(&".git".to_string()));
}

#[test]
fn test_writable_root_git_pointer_invalid_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // Create .git with invalid content (no "gitdir:" prefix)
    std::fs::write(root.join(".git"), "not a valid pointer").expect("write");

    let wr = WritableRoot::new(root);
    // Should fall back to default subpaths
    assert_eq!(wr.readonly_subpaths, default_readonly_subpaths());
}

#[test]
fn test_writable_root_git_pointer_multiline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let gitdir = root.join("actual_gitdir");
    std::fs::create_dir_all(&gitdir).expect("create gitdir");

    // Multi-line content — only first line should be parsed
    std::fs::write(
        root.join(".git"),
        format!("gitdir: {}\nextra line\n", gitdir.display()),
    )
    .expect("write");

    let wr = WritableRoot::new(root);
    let gitdir_rel = gitdir
        .strip_prefix(root)
        .expect("strip")
        .display()
        .to_string();
    assert!(
        wr.readonly_subpaths.contains(&gitdir_rel),
        "Multi-line pointer should resolve correctly"
    );
}
