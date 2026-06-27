use super::*;
use std::fs;
use tempfile::TempDir;

fn default_config() -> IgnoreConfig {
    IgnoreConfig::default()
}

fn setup_gitignore(dir: &Path, content: &str) {
    fs::write(dir.join(".gitignore"), content).expect("write .gitignore");
}

fn setup_ignore(dir: &Path, content: &str) {
    fs::write(dir.join(".ignore"), content).expect("write .ignore");
}

fn setup_agentignore(dir: &Path, content: &str) {
    fs::write(dir.join(".agentignore"), content).expect("write .agentignore");
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, "").expect("touch file");
}

#[test]
fn test_agentignore_hides_checked_in_secrets() {
    // `.agentignore` is for AI-only exclusions — files that are
    // intentionally checked in (so NOT in .gitignore) but the user
    // wants hidden from agent tooling.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup_agentignore(root, "secrets.toml\nfixtures/\n");
    touch(&root.join("secrets.toml"));
    touch(&root.join("fixtures").join("data.json"));
    touch(&root.join("main.rs"));

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&root.join("secrets.toml")));
    assert!(checker.is_ignored(&root.join("fixtures").join("data.json")));
    assert!(!checker.is_ignored(&root.join("main.rs")));
}

#[test]
fn test_agentignore_respects_disable_flag() {
    // When `respect_ignore` is false, `.agentignore` is also ignored
    // — they share the same gate (same conceptual category of
    // "additional ignore files").
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup_agentignore(root, "secret.txt\n");
    touch(&root.join("secret.txt"));

    let config = IgnoreConfig::default().with_ignore(false);
    let checker = PathChecker::new(root, &config);
    assert!(!checker.is_ignored(&root.join("secret.txt")));
}

#[test]
fn test_gitignore_basic_patterns() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    setup_gitignore(root, "*.log\ntarget/\n");
    touch(&root.join("app.log"));
    touch(&root.join("main.rs"));
    touch(&root.join("target").join("debug").join("app"));

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&root.join("app.log")));
    assert!(!checker.is_ignored(&root.join("main.rs")));
    assert!(checker.is_ignored(&root.join("target").join("debug").join("app")));
}

#[test]
fn test_gitignore_negation_pattern() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    setup_gitignore(root, "*.log\n!important.log\n");
    touch(&root.join("debug.log"));
    touch(&root.join("important.log"));

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&root.join("debug.log")));
    assert!(!checker.is_ignored(&root.join("important.log")));
}

#[test]
fn test_nested_gitignore_precedence() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Root ignores *.txt
    setup_gitignore(root, "*.txt\n");

    // Nested dir un-ignores *.txt
    let sub = root.join("docs");
    fs::create_dir_all(&sub).unwrap();
    setup_gitignore(&sub, "!*.txt\n");

    touch(&root.join("notes.txt"));
    touch(&sub.join("readme.txt"));

    let checker = PathChecker::new(root, &default_config());

    // Root .gitignore should ignore *.txt at root level
    assert!(checker.is_ignored(&root.join("notes.txt")));
    // Nested .gitignore should un-ignore *.txt in docs/
    assert!(!checker.is_ignored(&sub.join("readme.txt")));
}

#[test]
fn test_ignore_file_support() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Use .ignore file (ripgrep native)
    setup_ignore(root, "*.tmp\n");
    touch(&root.join("cache.tmp"));
    touch(&root.join("main.rs"));

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&root.join("cache.tmp")));
    assert!(!checker.is_ignored(&root.join("main.rs")));
}

#[test]
fn test_hidden_files_filtered_by_default() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    touch(&root.join(".hidden_file"));
    touch(&root.join("visible_file"));

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&root.join(".hidden_file")));
    assert!(!checker.is_ignored(&root.join("visible_file")));
}

#[test]
fn test_hidden_files_included_when_configured() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    touch(&root.join(".hidden_file"));

    let config = IgnoreConfig::default().with_hidden(true);
    let checker = PathChecker::new(root, &config);

    assert!(!checker.is_ignored(&root.join(".hidden_file")));
}

#[test]
fn test_default_patterns_node_modules() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let nm = root.join("node_modules").join("pkg").join("index.js");
    touch(&nm);

    let checker = PathChecker::new(root, &default_config());

    assert!(checker.is_ignored(&nm));
}

#[test]
fn test_custom_excludes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    touch(&root.join("output.csv"));
    touch(&root.join("main.rs"));

    let config = IgnoreConfig::default().with_excludes(vec!["*.csv".to_string()]);
    let checker = PathChecker::new(root, &config);

    assert!(checker.is_ignored(&root.join("output.csv")));
    assert!(!checker.is_ignored(&root.join("main.rs")));
}

#[test]
fn test_gitignore_disabled() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    setup_gitignore(root, "*.log\n");
    touch(&root.join("app.log"));

    let config = IgnoreConfig::default().with_gitignore(false);
    let checker = PathChecker::new(root, &config);

    // .gitignore should be ignored when respect_gitignore is false
    // But .log still won't match because there's no .ignore file either
    // (hidden filter is still on by default)
    assert!(!checker.is_ignored(&root.join("app.log")));
}

#[test]
fn test_filter_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    setup_gitignore(root, "*.log\n");
    touch(&root.join("app.log"));
    touch(&root.join("main.rs"));
    touch(&root.join("lib.rs"));

    let checker = PathChecker::new(root, &default_config());

    let paths = vec![
        root.join("app.log"),
        root.join("main.rs"),
        root.join("lib.rs"),
    ];

    let filtered = checker.filter_paths(&paths);
    assert_eq!(filtered.len(), 2);
    assert!(filtered.contains(&root.join("main.rs").as_path()));
    assert!(filtered.contains(&root.join("lib.rs").as_path()));
}

#[test]
fn test_directory_only_patterns() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Pattern with trailing / only matches directories
    setup_gitignore(root, "build/\n");
    let build_dir = root.join("build");
    fs::create_dir_all(&build_dir).unwrap();
    touch(&build_dir.join("output.js"));

    let checker = PathChecker::new(root, &default_config());

    // The directory itself should be ignored
    assert!(checker.is_ignored(&build_dir));
    // Files inside should also be ignored (via parent check)
    assert!(checker.is_ignored(&build_dir.join("output.js")));
}

#[test]
fn test_empty_config_no_filtering() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    setup_gitignore(root, "*.log\n");
    touch(&root.join("app.log"));

    let config = IgnoreConfig::ignoring_none();
    let checker = PathChecker::new(root, &config);

    // With ignoring_none, even .gitignore rules should not apply
    // (but default hardcoded patterns still apply)
    assert!(!checker.is_ignored(&root.join("app.log")));
}

#[test]
fn test_debug_impl() {
    let tmp = TempDir::new().unwrap();
    let checker = PathChecker::new(tmp.path(), &default_config());
    let debug_str = format!("{checker:?}");
    assert!(debug_str.contains("PathChecker"));
    assert!(debug_str.contains("matchers_count"));
}
