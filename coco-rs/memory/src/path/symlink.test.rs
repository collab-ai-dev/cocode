use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn returns_canonical_for_existing_path() {
    let temp = tempdir().unwrap();
    let real = temp.path().canonicalize().unwrap();
    assert_eq!(realpath_deepest_existing(temp.path()), Some(real));
}

#[test]
fn walks_up_for_non_existent_leaf() {
    let temp = tempdir().unwrap();
    let canonical = temp.path().canonicalize().unwrap();
    let path = temp.path().join("does/not/exist/yet.md");
    let resolved = realpath_deepest_existing(&path).expect("should resolve");
    assert_eq!(resolved, canonical.join("does/not/exist/yet.md"));
}
