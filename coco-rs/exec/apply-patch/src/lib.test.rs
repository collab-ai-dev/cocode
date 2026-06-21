use super::*;
use coco_exec_server::LOCAL_FS;
use coco_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use std::fs;
use std::string::ToString;
use tempfile::tempdir;

/// Helper to construct a patch with the given body.
fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

#[test]
fn test_collect_path_effects_add_delete_update_move() {
    let dir = tempdir().unwrap();
    let cwd = AbsolutePathBuf::from_absolute_path(dir.path()).unwrap();
    let absolute_delete = dir.path().join("absolute-delete.txt");
    let patch = wrap_patch(&format!(
        r#"*** Add File: add.txt
+add
*** Delete File: {}
*** Update File: update.txt
@@
-old
+new
*** Update File: old-name.txt
*** Move to: nested/new-name.txt
@@
-old_name
+new_name"#,
        absolute_delete.display(),
    ));
    let parsed = parse_patch(&patch).unwrap();

    let effects = collect_path_effects(&parsed.hunks, &cwd);

    let expected = vec![
        dir.path().join("absolute-delete.txt"),
        dir.path().join("add.txt"),
        dir.path().join("nested/new-name.txt"),
        dir.path().join("old-name.txt"),
        dir.path().join("update.txt"),
    ];
    assert_eq!(effects.permission_paths, expected);
    assert_eq!(effects.history_paths, expected);
    assert_eq!(effects.lsp_notify_paths, expected);
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
async fn test_apply_patch_hunks_accept_relative_and_absolute_paths() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().abs();
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

    apply_patch(&patch, &cwd, &mut stdout, &mut stderr, LOCAL_FS.as_ref())
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
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

#[tokio::test]
async fn test_unified_diff() {
    // Start with a file containing four lines.
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi.txt");
    fs::write(&path, "foo\nbar\nbaz\nqux\n").unwrap();
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
    let patch = parse_patch(&patch).unwrap();

    let update_file_chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };
    let path_abs = path.as_path().abs();
    let diff = unified_diff_from_chunks(&path_abs, update_file_chunks, LOCAL_FS.as_ref())
        .await
        .unwrap();
    let expected_diff = r#"@@ -1,4 +1,4 @@
 foo
-bar
+BAR
 baz
-qux
+QUX
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        content: "foo\nBAR\nbaz\nQUX\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_first_line_replacement() {
    // Replace the very first line of the file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("first.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
-foo
+FOO
 bar
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_abs = path.as_path().abs();
    let diff = unified_diff_from_chunks(&path_abs, chunks, LOCAL_FS.as_ref())
        .await
        .unwrap();
    let expected_diff = r#"@@ -1,2 +1,2 @@
-foo
+FOO
 bar
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        content: "FOO\nbar\nbaz\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_last_line_replacement() {
    // Replace the very last line of the file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("last.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 foo
 bar
-baz
+BAZ
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_abs = path.as_path().abs();
    let diff = unified_diff_from_chunks(&path_abs, chunks, LOCAL_FS.as_ref())
        .await
        .unwrap();
    let expected_diff = r#"@@ -2,2 +2,2 @@
 bar
-baz
+BAZ
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        content: "foo\nbar\nBAZ\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_insert_at_eof() {
    // Insert a new line at end‑of‑file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("insert.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
+quux
*** End of File
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_abs = path.as_path().abs();
    let diff = unified_diff_from_chunks(&path_abs, chunks, LOCAL_FS.as_ref())
        .await
        .unwrap();
    let expected_diff = r#"@@ -3 +3,2 @@
 baz
+quux
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        content: "foo\nbar\nbaz\nquux\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_interleaved_changes() {
    // Original file with six lines.
    let dir = tempdir().unwrap();
    let path = dir.path().join("interleaved.txt");
    fs::write(&path, "a\nb\nc\nd\ne\nf\n").unwrap();

    // Patch replaces two separate lines and appends a new one at EOF using
    // three distinct chunks.
    let patch_body = format!(
        r#"*** Update File: {}
@@
 a
-b
+B
@@
 d
-e
+E
@@
 f
+g
*** End of File"#,
        path.display()
    );
    let patch = wrap_patch(&patch_body);

    // Extract chunks then build the unified diff.
    let parsed = parse_patch(&patch).unwrap();
    let chunks = match parsed.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_abs = path.as_path().abs();
    let diff = unified_diff_from_chunks(&path_abs, chunks, LOCAL_FS.as_ref())
        .await
        .unwrap();

    let expected_diff = r#"@@ -1,6 +1,7 @@
 a
-b
+B
 c
 d
-e
+E
 f
+g
"#;

    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        content: "a\nB\nc\nd\nE\nf\ng\n".to_string(),
    };

    assert_eq!(expected, diff);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
    )
    .await
    .unwrap();
    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(
        contents,
        r#"a
B
c
d
E
f
g
"#
    );
}

#[tokio::test]
async fn test_apply_patch_fails_on_write_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("readonly.txt");
    fs::write(&path, "before\n").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms).unwrap();

    let patch = wrap_patch(&format!(
        "*** Update File: {}\n@@\n-before\n+after\n*** End Patch",
        path.display()
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = apply_patch(
        &patch,
        &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
    )
    .await;
    assert!(result.is_err());
}
