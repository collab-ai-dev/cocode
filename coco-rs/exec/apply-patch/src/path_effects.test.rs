use std::fs;

use coco_exec_server::LOCAL_FS;
use coco_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::parse_patch;

fn cwd(dir: &TempDir) -> PathUri {
    PathUri::from_path(dir.path()).expect("temporary directory is absolute")
}

#[test]
fn collects_move_source_and_destination_once() {
    let dir = TempDir::new().expect("create temp directory");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** End Patch",
    )
    .expect("valid patch");

    let effects = collect_path_effects(&parsed.hunks, &cwd(&dir)).expect("resolve paths");
    let paths = effects
        .paths()
        .iter()
        .map(PathUri::inferred_native_path_string)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            dir.path().join("old.txt").display().to_string(),
            dir.path().join("new.txt").display().to_string(),
        ]
    );
}

#[tokio::test]
async fn rejects_move_destination_used_by_another_hunk() {
    let dir = TempDir::new().expect("create temp directory");
    fs::write(dir.path().join("old.txt"), "old\n").expect("write source");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** Add File: new.txt\n+duplicate\n*** End Patch",
    )
    .expect("valid patch");

    let error = validate_hunk_paths(&parsed.hunks, &cwd(&dir), LOCAL_FS.as_ref(), None)
        .await
        .expect_err("destination collision must fail");
    assert!(error.to_string().contains("multiple operations target"));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_aliases_of_the_same_source() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    fs::write(dir.path().join("real.txt"), "old\n").expect("write source");
    symlink("real.txt", dir.path().join("alias.txt")).expect("create symlink");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Delete File: real.txt\n*** Delete File: alias.txt\n*** End Patch",
    )
    .expect("valid patch");

    let error = validate_hunk_paths(&parsed.hunks, &cwd(&dir), LOCAL_FS.as_ref(), None)
        .await
        .expect_err("filesystem aliases must fail");
    assert!(error.to_string().contains("multiple operations target"));
}

#[cfg(unix)]
#[tokio::test]
async fn validated_effects_use_canonical_paths_through_symlinked_parents() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    let target = TempDir::new().expect("create symlink target");
    symlink(target.path(), dir.path().join("linked")).expect("create directory symlink");
    let parsed = parse_patch("*** Begin Patch\n*** Add File: linked/new.txt\n+new\n*** End Patch")
        .expect("valid patch");

    let effects = validate_hunk_paths(&parsed.hunks, &cwd(&dir), LOCAL_FS.as_ref(), None)
        .await
        .expect("validate paths");

    assert_eq!(
        effects.paths(),
        vec![PathUri::from_path(target.path().join("new.txt")).expect("absolute target path")]
    );
}
