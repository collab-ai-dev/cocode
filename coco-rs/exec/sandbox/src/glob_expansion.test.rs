use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn looks_like_glob_detects_metacharacters() {
    assert!(looks_like_glob("**/*.env"));
    assert!(looks_like_glob("foo?bar"));
    assert!(looks_like_glob("[abc]"));
    assert!(!looks_like_glob("/abs/path/file.env"));
    assert!(!looks_like_glob("relative/path"));
}

#[test]
fn expand_returns_empty_for_no_globs_or_zero_depth() {
    let tmp = TempDir::new().unwrap();
    let roots = vec![tmp.path().to_path_buf()];
    assert!(expand(&roots, &[], 5).is_empty(), "no globs → empty");
    assert!(
        expand(&roots, &["*.env".to_string()], 0).is_empty(),
        "depth 0 → empty",
    );
}

#[test]
fn expand_matches_leaf_pattern_under_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(".env"), "SECRET=1").unwrap();
    std::fs::write(root.join("not_secret.txt"), "ok").unwrap();
    let matches = expand(&[root.to_path_buf()], &["*.env".to_string()], 3);
    assert_eq!(matches.len(), 1);
    assert!(matches[0].ends_with(".env"));
}

#[test]
fn expand_respects_max_depth() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::write(root.join("a/b/c/secret.env"), "x").unwrap();
    // Depth 2 keeps walker shallow — the file at depth 4 is invisible.
    let shallow = expand(&[root.to_path_buf()], &["**/*.env".to_string()], 2);
    assert!(
        shallow.is_empty(),
        "depth 2 should not reach a/b/c/secret.env"
    );
    // Depth 5 reaches it.
    let deep = expand(&[root.to_path_buf()], &["**/*.env".to_string()], 5);
    assert_eq!(deep.len(), 1);
}

#[test]
fn expand_invalid_pattern_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    // `[` without closing bracket is a malformed glob — globset rejects it.
    let matches = expand(
        &[root.to_path_buf()],
        &["[unclosed".to_string(), "*.txt".to_string()],
        3,
    );
    // The valid pattern still expands.
    assert_eq!(matches.len(), 1);
    assert!(matches[0].ends_with("a.txt"));
}

#[test]
fn merge_paths_and_globs_dedupes_overlap() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("dup.env"), "x").unwrap();
    let explicit = vec![root.join("dup.env")];
    let merged = merge_paths_and_globs(explicit, &["*.env".to_string()], &[root.to_path_buf()], 3);
    assert_eq!(merged.len(), 1, "explicit + glob match the same path");
    assert!(merged[0].ends_with("dup.env"));
}
