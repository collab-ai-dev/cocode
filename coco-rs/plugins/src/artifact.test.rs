use super::*;

#[test]
fn inspection_is_stable_and_covers_content() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(root.path().join("nested")).expect("mkdir");
    std::fs::write(root.path().join("PLUGIN.toml"), "name = \"demo\"\n").expect("write");
    std::fs::write(root.path().join("nested/skill.md"), "one").expect("write");

    let first = inspect_artifact(root.path()).expect("inspect");
    let same = inspect_artifact(root.path()).expect("inspect again");
    assert_eq!(first, same);
    assert_eq!(first.file_count, 2);

    std::fs::write(root.path().join("nested/skill.md"), "two").expect("rewrite");
    let changed = inspect_artifact(root.path()).expect("inspect changed");
    assert_ne!(first.tree_sha256, changed.tree_sha256);
}

#[cfg(unix)]
#[test]
fn inspection_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("secret"), "secret").expect("write");
    symlink(outside.path().join("secret"), root.path().join("linked")).expect("symlink");

    let error = inspect_artifact(root.path()).expect_err("symlink must fail");
    assert!(error.to_string().contains("symlink"));
}
