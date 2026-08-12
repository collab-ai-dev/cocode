use super::*;
use coco_exec_server::LOCAL_FS;
use pretty_assertions::assert_eq;
use std::fs;
use tempfile::tempdir;

/// Helper to construct a patch with the given body.
fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

#[tokio::test]
async fn test_add_file_hunk_creates_file_with_contents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("add.txt");
    let patch = wrap_patch(&format!(
        r#"*** Add File: {}
+ab
+cd"#,
        path.display()
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    // Verify expected stdout and stderr outputs.
    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nA {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");
    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(contents, "ab\ncd\n");
}

#[tokio::test]
async fn raw_apply_rejects_unroutable_environment_header() {
    let dir = tempdir().unwrap();
    let patch = "*** Begin Patch\n*** Environment ID: remote\n*** Add File: wrong.txt\n+wrong\n*** End Patch";
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let error = apply_patch(
        patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        None,
    )
    .await
    .expect_err("environment selector must fail closed");

    assert!(
        error
            .to_string()
            .contains("environment selection is unavailable")
    );
    assert!(!dir.path().join("wrong.txt").exists());
}

#[tokio::test]
async fn test_apply_patch_hunks_accept_relative_and_absolute_paths() {
    let dir = tempdir().unwrap();
    let cwd = PathUri::from_path(dir.path()).expect("absolute test path");
    let relative_add = dir.path().join("relative-add.txt");
    let absolute_add = dir.path().join("absolute-add.txt");
    let relative_delete = dir.path().join("relative-delete.txt");
    let absolute_delete = dir.path().join("absolute-delete.txt");
    let relative_update = dir.path().join("relative-update.txt");
    let absolute_update = dir.path().join("absolute-update.txt");
    fs::write(&relative_delete, "delete relative\n").unwrap();
    fs::write(&absolute_delete, "delete absolute\n").unwrap();
    fs::write(&relative_update, "relative old\n").unwrap();
    fs::write(&absolute_update, "absolute old\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Add File: relative-add.txt
+relative add
*** Add File: {}
+absolute add
*** Delete File: relative-delete.txt
*** Delete File: {}
*** Update File: relative-update.txt
@@
-relative old
+relative new
*** Update File: {}
@@
-absolute old
+absolute new"#,
        absolute_add.display(),
        absolute_delete.display(),
        absolute_update.display(),
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    apply_patch(
        &patch,
        &cwd,
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    assert_eq!(fs::read_to_string(&relative_add).unwrap(), "relative add\n");
    assert_eq!(fs::read_to_string(&absolute_add).unwrap(), "absolute add\n");
    assert!(!relative_delete.exists());
    assert!(!absolute_delete.exists());
    assert_eq!(
        fs::read_to_string(&relative_update).unwrap(),
        "relative new\n"
    );
    assert_eq!(
        fs::read_to_string(&absolute_update).unwrap(),
        "absolute new\n"
    );
    assert_eq!(String::from_utf8(stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(stdout).unwrap(),
        format!(
            "Success. Updated the following files:\nA relative-add.txt\nA {}\nM relative-update.txt\nM {}\nD relative-delete.txt\nD {}\n",
            absolute_add.display(),
            absolute_update.display(),
            absolute_delete.display(),
        )
    );
}

#[tokio::test]
async fn test_delete_file_hunk_removes_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("del.txt");
    fs::write(&path, "x").unwrap();
    let patch = wrap_patch(&format!("*** Delete File: {}", path.display()));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nD {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");
    assert!(!path.exists());
}

#[tokio::test]
async fn test_update_file_hunk_modifies_content() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("update.txt");
    fs::write(&path, "foo\nbar\n").unwrap();
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 foo
-bar
+baz"#,
        path.display()
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    // Validate modified file contents and expected stdout/stderr.
    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nM {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");
    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "foo\nbaz\n");
}

#[tokio::test]
async fn test_update_file_hunk_can_move_file() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dest = dir.path().join("dst.txt");
    fs::write(&src, "line\n").unwrap();
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
*** Move to: {}
@@
-line
+line2"#,
        src.display(),
        dest.display()
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    // Validate move semantics and expected stdout/stderr.
    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nM {}\n",
        dest.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");
    assert!(!src.exists());
    let contents = fs::read_to_string(&dest).unwrap();
    assert_eq!(contents, "line2\n");
}

#[cfg(unix)]
#[tokio::test]
async fn test_failed_move_returns_committed_destination_delta() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let source_dir = dir.path().join("locked");
    let dest_dir = dir.path().join("out");
    fs::create_dir(&source_dir).unwrap();
    fs::create_dir(&dest_dir).unwrap();
    let src = source_dir.join("src.txt");
    let dest = dest_dir.join("dst.txt");
    fs::write(&src, "line\n").unwrap();
    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let patch =
        wrap_patch("*** Update File: locked/src.txt\n*** Move to: out/dst.txt\n@@\n-line\n+line2");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let failure = apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .expect_err("source removal should fail after destination write");

    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains(&format!("Failed to remove original {}", src.display()))
    );
    assert_eq!(
        failure.delta(),
        &AppliedPatchDelta::new(
            vec![AppliedPatchChange {
                path: PathUri::from_path(&dest).expect("absolute destination path"),
                change: AppliedPatchFileChange::Add {
                    content: "line2\n".to_string(),
                    overwritten_content: None,
                },
            }],
            /*exact*/ true,
        )
    );
    assert_eq!(fs::read_to_string(src).unwrap(), "line\n");
    assert_eq!(fs::read_to_string(dest).unwrap(), "line2\n");
}

/// Verify that a single `Update File` hunk with multiple change chunks can update different
/// parts of a file and that the file is listed only once in the summary.
#[tokio::test]
async fn test_multiple_update_chunks_apply_to_single_file() {
    // Start with a file containing four lines.
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi.txt");
    fs::write(&path, "foo\nbar\nbaz\nqux\n").unwrap();
    // Construct an update patch with two separate change chunks.
    // The first chunk uses the line `foo` as context and transforms `bar` into `BAR`.
    // The second chunk uses `baz` as context and transforms `qux` into `QUX`.
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 foo
-bar
+BAR
@@
 baz
-qux
+QUX"#,
        path.display()
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nM {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");
    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "foo\nBAR\nbaz\nQUX\n");
}

/// A more involved `Update File` hunk that exercises additions, deletions and
/// replacements in separate chunks that appear in non‑adjacent parts of the
/// file.  Verifies that all edits are applied and that the summary lists the
/// file only once.
#[tokio::test]
async fn test_update_file_hunk_interleaved_changes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("interleaved.txt");

    // Original file: six numbered lines.
    fs::write(&path, "a\nb\nc\nd\ne\nf\n").unwrap();

    // Patch performs:
    //  • Replace `b` → `B`
    //  • Replace `e` → `E` (using surrounding context)
    //  • Append new line `g` at the end‑of‑file
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 a
-b
+B
@@
 c
 d
-e
+E
@@
 f
+g
*** End of File"#,
        path.display()
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    let stdout_str = String::from_utf8(stdout).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();

    let expected_out = format!(
        "Success. Updated the following files:\nM {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);
    assert_eq!(stderr_str, "");

    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "a\nB\nc\nd\nE\nf\ng\n");
}

#[tokio::test]
async fn test_pure_addition_chunk_followed_by_removal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("panic.txt");
    fs::write(&path, "line1\nline2\nline3\n").unwrap();
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
+after-context
+second-line
@@
 line1
-line2
-line3
+line2-replacement"#,
        path.display()
    ));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(
        contents,
        "line1\nline2-replacement\nafter-context\nsecond-line\n"
    );
}

/// Ensure that patches authored with ASCII characters can update lines that
/// contain typographic Unicode punctuation (e.g. EN DASH, NON-BREAKING
/// HYPHEN). Historically `git apply` succeeds in such scenarios but our
/// internal matcher failed requiring an exact byte-for-byte match.  The
/// fuzzy-matching pass that normalises common punctuation should now bridge
/// the gap.
#[tokio::test]
async fn test_update_line_with_unicode_dash() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("unicode.py");

    // Original line contains EN DASH (\u{2013}) and NON-BREAKING HYPHEN (\u{2011}).
    let original = "import asyncio  # local import \u{2013} avoids top\u{2011}level dep\n";
    std::fs::write(&path, original).unwrap();

    // Patch uses plain ASCII dash / hyphen.
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
-import asyncio  # local import - avoids top-level dep
+import asyncio  # HELLO"#,
        path.display()
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    // File should now contain the replaced comment.
    let expected = "import asyncio  # HELLO\n";
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, expected);

    // Ensure success summary lists the file as modified.
    let stdout_str = String::from_utf8(stdout).unwrap();
    let expected_out = format!(
        "Success. Updated the following files:\nM {}\n",
        path.display()
    );
    assert_eq!(stdout_str, expected_out);

    // No stderr expected.
    assert_eq!(String::from_utf8(stderr).unwrap(), "");
}

#[cfg(unix)]
#[tokio::test]
async fn test_apply_patch_fails_on_write_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let locked_dir = dir.path().join("locked");
    fs::create_dir(&locked_dir).unwrap();
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let patch = wrap_patch("*** Add File: locked/new.txt\n+after");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await;
    let failure = result.expect_err("write should fail");

    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!failure.delta().is_exact());
}

#[tokio::test]
async fn test_unreadable_destinations_return_inexact_delta() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("binary.dat");
    fs::write(dir.path().join("source.txt"), "before\n").unwrap();
    let cwd = PathUri::from_path(dir.path()).expect("absolute test path");

    for patch in [
        wrap_patch("*** Add File: binary.dat\n+text"),
        wrap_patch("*** Update File: source.txt\n*** Move to: binary.dat\n@@\n-before\n+after"),
    ] {
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let delta = apply_patch(
            &patch,
            &cwd,
            &mut stdout,
            &mut stderr,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap();

        assert!(!delta.is_exact());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_delete_symlink_returns_inexact_delta() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    fs::write(dir.path().join("target.txt"), "target\n").unwrap();
    symlink(dir.path().join("target.txt"), dir.path().join("link.txt")).unwrap();
    let patch = wrap_patch("*** Delete File: link.txt");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let delta = apply_patch(
        &patch,
        &PathUri::from_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    assert!(!delta.is_exact());
}
