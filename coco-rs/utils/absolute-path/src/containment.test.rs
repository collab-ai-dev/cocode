use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn realpath_returns_canonical_for_existing_path() {
    let temp = tempdir().unwrap();
    let real = temp.path().canonicalize().unwrap();
    assert_eq!(realpath_deepest_existing(temp.path()), Some(real));
}

#[test]
fn realpath_walks_up_for_non_existent_leaf() {
    let temp = tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let path = temp.path().join("does/not/exist/yet.md");
    let resolved = realpath_deepest_existing(&path).expect("should resolve");
    assert_eq!(resolved, canonical.join("does/not/exist/yet.md"));
}

#[test]
fn realpath_collapses_dotdot_in_nonexistent_tail() {
    let temp = tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    // `a` doesn't exist; the `..` must be applied (not dropped), so the result
    // is <temp>/b.md — never <temp>/a/b.md (a dropped `..`) which would then
    // deceptively appear contained.
    let path = temp.path().join("a/../b.md");
    assert_eq!(
        realpath_deepest_existing(&path),
        Some(canonical.join("b.md"))
    );
    // A `..` that walks above an existing dir escapes it.
    let escape = temp.path().join("a/../../outside.md");
    let parent = canonical.parent().unwrap();
    assert_eq!(
        realpath_deepest_existing(&escape),
        Some(parent.join("outside.md"))
    );
}

#[test]
fn lexical_normalize_collapses_dot_and_parent() {
    assert_eq!(
        lexical_normalize(Path::new("/a/b/../c/./d")),
        PathBuf::from("/a/c/d")
    );
}

#[test]
fn contains_root_itself_and_descendant() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    assert!(contains_symlink_aware(root, root));
    assert!(contains_symlink_aware(root, &root.join("child/leaf.md")));
}

#[test]
fn contains_rejects_sibling() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let sibling = temp.path().join("sibling/file.md");
    assert!(!contains_symlink_aware(&root, &sibling));
}

#[test]
fn contains_rejects_traversal_escape() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    // `<root>/../secret` normalizes to `<temp>/secret`, outside root.
    let escape = root.join("../secret.md");
    assert!(!contains_symlink_aware(&root, &escape));
}

#[test]
fn contains_hypothetical_paths_use_lexical_when_neither_exists() {
    let root = Path::new("/nonexistent/root");
    assert!(contains_symlink_aware(
        root,
        Path::new("/nonexistent/root/a.md")
    ));
    assert!(!contains_symlink_aware(
        root,
        Path::new("/nonexistent/other/a.md")
    ));
}

/// A `..` escape must be denied even when neither side exists on disk: the
/// resolver re-appends the non-existent tail verbatim, so the surviving `..`
/// has to be collapsed lexically *after* resolution or it deceptively
/// `starts_with` the root. (Regression: resolving the raw path alone let
/// `<root>/a/../../x` pass.)
#[test]
fn contains_rejects_dotdot_escape_when_neither_exists() {
    let root = Path::new("/memdir");
    // /memdir/nested/../../outside.md escapes to /outside.md.
    assert!(!contains_symlink_aware(
        root,
        Path::new("/memdir/nested/../../outside.md")
    ));
    // A `..` that stays inside is still contained.
    assert!(contains_symlink_aware(
        root,
        Path::new("/memdir/nested/../keep.md")
    ));
}

/// A symlink rooted inside `root` that resolves OUTSIDE must be rejected.
#[cfg(unix)]
#[test]
fn contains_rejects_real_symlink_escape() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let escape_target = temp.path().join("escape-target");
    std::fs::write(&escape_target, "secret").unwrap();
    let escape_link = root.join("escape");
    std::os::unix::fs::symlink(&escape_target, &escape_link).unwrap();
    assert!(
        !contains_symlink_aware(&root, &escape_link),
        "symlink rooted inside root but resolving outside must be rejected"
    );
}

/// A `..` that cancels a symlink component must NOT be collapsed lexically:
/// the check has to resolve `link` the way the kernel does at read time.
/// `<root>/link/../secret` with `link -> <outside>` escapes to
/// `<outside-parent>/secret`, so it must be rejected even though the lexical
/// form `<root>/secret` looks contained. Regression for the symlink+`..`
/// containment bypass.
#[cfg(unix)]
#[test]
fn contains_rejects_dotdot_through_symlink_escape() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    // `link` -> <temp>/outside/sub (a real dir outside root), so the kernel
    // resolves `link/../secret.md` to <temp>/outside/secret.md.
    std::fs::create_dir_all(temp.path().join("outside/sub")).unwrap();
    std::fs::write(temp.path().join("outside/secret.md"), "SECRET").unwrap();
    std::os::unix::fs::symlink(temp.path().join("outside/sub"), root.join("link")).unwrap();
    let candidate = root.join("link/../secret.md");
    assert!(
        !contains_symlink_aware(&root, &candidate),
        "`..` through a symlink must resolve with kernel semantics and be rejected"
    );
}

/// A `..` that stays inside `root` (cancelling a genuine subdirectory, not a
/// symlink) must still be allowed — the fix must not over-deny legit imports.
#[cfg(unix)]
#[test]
fn contains_allows_dotdot_that_stays_inside_root() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("keep.md"), "ok").unwrap();
    // <root>/sub/../keep.md resolves back to <root>/keep.md — still contained.
    let candidate = root.join("sub/../keep.md");
    assert!(contains_symlink_aware(&root, &candidate));
}

/// A dangling symlink inside `root` triggers the asymmetric fail-closed branch.
#[cfg(unix)]
#[test]
fn contains_rejects_dangling_symlink_inside_root() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let dangling = root.join("dangling");
    std::os::unix::fs::symlink("/nonexistent/path/that/will/never/exist", &dangling).unwrap();
    assert!(
        !contains_symlink_aware(&root, &dangling),
        "dangling symlink inside root must fail closed"
    );
}
