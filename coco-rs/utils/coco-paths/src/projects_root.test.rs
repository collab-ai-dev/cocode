use super::*;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn projects_root_layout() {
    assert_eq!(
        projects_root(Path::new("/x")),
        std::path::PathBuf::from("/x/projects"),
    );
}

#[test]
fn project_dir_path_computation() {
    assert_eq!(
        project_dir(Path::new("/x"), Path::new("/a/b")),
        std::path::PathBuf::from("/x/projects/-a-b"),
    );
}

#[test]
fn find_returns_exact_match_when_dir_exists() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let expected = project_dir(root, Path::new("/foo/bar"));
    std::fs::create_dir_all(&expected).unwrap();

    let found = find_project_dir(root, Path::new("/foo/bar")).unwrap();
    assert_eq!(found, Some(expected));
}

#[test]
fn find_returns_none_when_short_slug_dir_missing() {
    let tmp = tempdir().unwrap();
    let found = find_project_dir(tmp.path(), Path::new("/foo/bar")).unwrap();
    assert_eq!(found, None);
}

#[test]
fn find_falls_back_to_prefix_match_for_long_path() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Build a path that sanitises to >200 bytes, then plant a dir
    // whose name shares the 200-byte prefix but has a *different*
    // hash suffix (simulating the Bun/Node hash divergence).
    //
    // Input `/aaa...aaa` (1 `/` + 250 `a`s = 251 chars) sanitises to
    // `-aaa...aaa` (1 `-` + 250 `a`s). After 200-byte truncation
    // the real on-disk prefix is `-` + 199 `a`s, not 200 plain `a`s.
    let long_segment = "a".repeat(250);
    let long_path_str = format!("/{long_segment}");
    let long_path = Path::new(&long_path_str);

    let prefix = format!("-{}", "a".repeat(MAX_SANITIZED_LENGTH - 1));
    let fake_hash = "deadbeef";
    let prefix_with_diff_hash = format!("{prefix}-{fake_hash}");
    let planted_dir = projects_root(root).join(&prefix_with_diff_hash);
    std::fs::create_dir_all(&planted_dir).unwrap();

    let found = find_project_dir(root, long_path).unwrap();
    assert_eq!(found, Some(planted_dir));
}

#[test]
fn find_handles_missing_projects_root_gracefully() {
    let tmp = tempdir().unwrap();
    // tmp.path()/projects does not exist — should return None, not error.
    let found = find_project_dir(tmp.path(), Path::new("/foo/bar")).unwrap();
    assert_eq!(found, None);
}
