use super::*;
use crate::unified_diff_from_chunks;
use assert_matches::assert_matches;
use coco_exec_server::LOCAL_FS;
use coco_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::PathBuf;
use std::string::ToString;
use tempfile::tempdir;

/// Helper to construct a patch with the given body.
fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

fn strs_to_strings(strs: &[&str]) -> Vec<String> {
    strs.iter().map(ToString::to_string).collect()
}

// Test helpers to reduce repetition when building bash -lc heredoc scripts
fn args_bash(script: &str) -> Vec<String> {
    strs_to_strings(&["bash", "-lc", script])
}

fn args_powershell(script: &str) -> Vec<String> {
    strs_to_strings(&["powershell.exe", "-Command", script])
}

fn args_powershell_no_profile(script: &str) -> Vec<String> {
    strs_to_strings(&["powershell.exe", "-NoProfile", "-Command", script])
}

fn args_pwsh(script: &str) -> Vec<String> {
    strs_to_strings(&["pwsh", "-NoProfile", "-Command", script])
}

fn args_cmd(script: &str) -> Vec<String> {
    strs_to_strings(&["cmd.exe", "/c", script])
}

fn heredoc_script(prefix: &str) -> String {
    format!(
        "{prefix}apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH"
    )
}

fn heredoc_script_ps(prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH{suffix}"
    )
}

fn expected_single_add() -> Vec<Hunk> {
    vec![Hunk::AddFile {
        path: PathBuf::from("foo"),
        contents: "hi\n".to_string(),
    }]
}

fn assert_match_args(args: Vec<String>, expected_workdir: Option<&str>) {
    match maybe_parse_apply_patch(&args) {
        MaybeApplyPatch::Body(ApplyPatchArgs { hunks, workdir, .. }) => {
            assert_eq!(workdir.as_deref(), expected_workdir);
            assert_eq!(hunks, expected_single_add());
        }
        result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
    }
}

fn assert_match(script: &str, expected_workdir: Option<&str>) {
    let args = args_bash(script);
    assert_match_args(args, expected_workdir);
}

fn assert_not_match(script: &str) {
    let args = args_bash(script);
    assert_matches!(
        maybe_parse_apply_patch(&args),
        MaybeApplyPatch::NotApplyPatch
    );
}

#[tokio::test]
async fn test_implicit_patch_single_arg_is_error() {
    let patch = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch".to_string();
    let args = vec![patch];
    let dir = tempdir().unwrap();
    assert_matches!(
        maybe_parse_apply_patch_verified(
            &args,
            &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
            LOCAL_FS.as_ref()
        )
        .await,
        MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation)
    );
}

#[tokio::test]
async fn test_implicit_patch_bash_script_is_error() {
    let script = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch";
    let args = args_bash(script);
    let dir = tempdir().unwrap();
    assert_matches!(
        maybe_parse_apply_patch_verified(
            &args,
            &AbsolutePathBuf::from_absolute_path(dir.path()).unwrap(),
            LOCAL_FS.as_ref()
        )
        .await,
        MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation)
    );
}

#[tokio::test]
async fn test_literal() {
    let args = strs_to_strings(&[
        "apply_patch",
        r#"*** Begin Patch
*** Add File: foo
+hi
*** End Patch
"#,
    ]);

    match maybe_parse_apply_patch(&args) {
        MaybeApplyPatch::Body(ApplyPatchArgs { hunks, .. }) => {
            assert_eq!(
                hunks,
                vec![Hunk::AddFile {
                    path: PathBuf::from("foo"),
                    contents: "hi\n".to_string()
                }]
            );
        }
        result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
    }
}

#[tokio::test]
async fn test_literal_applypatch() {
    let args = strs_to_strings(&[
        "applypatch",
        r#"*** Begin Patch
*** Add File: foo
+hi
*** End Patch
"#,
    ]);

    match maybe_parse_apply_patch(&args) {
        MaybeApplyPatch::Body(ApplyPatchArgs { hunks, .. }) => {
            assert_eq!(
                hunks,
                vec![Hunk::AddFile {
                    path: PathBuf::from("foo"),
                    contents: "hi\n".to_string()
                }]
            );
        }
        result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
    }
}

#[tokio::test]
async fn test_heredoc() {
    assert_match(&heredoc_script(""), /*expected_workdir*/ None);
}

#[tokio::test]
async fn test_heredoc_non_login_shell() {
    let script = heredoc_script("");
    let args = strs_to_strings(&["bash", "-c", &script]);
    assert_match_args(args, /*expected_workdir*/ None);
}

#[tokio::test]
async fn test_heredoc_applypatch() {
    let args = strs_to_strings(&[
        "bash",
        "-lc",
        r#"applypatch <<'PATCH'
*** Begin Patch
*** Add File: foo
+hi
*** End Patch
PATCH"#,
    ]);

    match maybe_parse_apply_patch(&args) {
        MaybeApplyPatch::Body(ApplyPatchArgs { hunks, workdir, .. }) => {
            assert_eq!(workdir, None);
            assert_eq!(
                hunks,
                vec![Hunk::AddFile {
                    path: PathBuf::from("foo"),
                    contents: "hi\n".to_string()
                }]
            );
        }
        result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
    }
}

#[tokio::test]
async fn test_powershell_heredoc() {
    let script = heredoc_script("");
    assert_match_args(args_powershell(&script), /*expected_workdir*/ None);
}
#[tokio::test]
async fn test_powershell_heredoc_no_profile() {
    let script = heredoc_script("");
    assert_match_args(
        args_powershell_no_profile(&script),
        /*expected_workdir*/ None,
    );
}
#[tokio::test]
async fn test_pwsh_heredoc() {
    let script = heredoc_script("");
    assert_match_args(args_pwsh(&script), /*expected_workdir*/ None);
}

#[tokio::test]
async fn test_cmd_heredoc_with_cd() {
    let script = heredoc_script("cd foo && ");
    assert_match_args(args_cmd(&script), Some("foo"));
}

#[tokio::test]
async fn test_heredoc_with_leading_cd() {
    assert_match(&heredoc_script("cd foo && "), Some("foo"));
}

#[tokio::test]
async fn test_cd_with_semicolon_is_ignored() {
    assert_not_match(&heredoc_script("cd foo; "));
}

#[tokio::test]
async fn test_cd_or_apply_patch_is_ignored() {
    assert_not_match(&heredoc_script("cd bar || "));
}

#[tokio::test]
async fn test_cd_pipe_apply_patch_is_ignored() {
    assert_not_match(&heredoc_script("cd bar | "));
}

#[tokio::test]
async fn test_cd_single_quoted_path_with_spaces() {
    assert_match(&heredoc_script("cd 'foo bar' && "), Some("foo bar"));
}

#[tokio::test]
async fn test_cd_double_quoted_path_with_spaces() {
    assert_match(&heredoc_script("cd \"foo bar\" && "), Some("foo bar"));
}

#[tokio::test]
async fn test_echo_and_apply_patch_is_ignored() {
    assert_not_match(&heredoc_script("echo foo && "));
}

#[tokio::test]
async fn test_apply_patch_with_arg_is_ignored() {
    let script =
        "apply_patch foo <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH";
    assert_not_match(script);
}

#[tokio::test]
async fn test_double_cd_then_apply_patch_is_ignored() {
    assert_not_match(&heredoc_script("cd foo && cd bar && "));
}

#[tokio::test]
async fn test_cd_two_args_is_ignored() {
    assert_not_match(&heredoc_script("cd foo bar && "));
}

#[tokio::test]
async fn test_cd_then_apply_patch_then_extra_is_ignored() {
    let script = heredoc_script_ps("cd bar && ", " && echo done");
    assert_not_match(&script);
}

#[tokio::test]
async fn test_echo_then_cd_and_apply_patch_is_ignored() {
    // Ensure preceding commands before the `cd && apply_patch <<...` sequence do not match.
    assert_not_match(&heredoc_script("echo foo; cd bar && "));
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
async fn test_apply_patch_should_resolve_absolute_paths_in_cwd() {
    let session_dir = tempdir().unwrap();
    let relative_path = "source.txt";

    // Note that we need this file to exist for the patch to be "verified"
    // and parsed correctly.
    let session_file_path = session_dir.path().join(relative_path);
    fs::write(&session_file_path, "session directory content\n").unwrap();

    let argv = vec![
        "apply_patch".to_string(),
        r#"*** Begin Patch
*** Update File: source.txt
@@
-session directory content
+updated session directory content
*** End Patch"#
            .to_string(),
    ];

    let result = maybe_parse_apply_patch_verified(
        &argv,
        &AbsolutePathBuf::from_absolute_path(session_dir.path()).unwrap(),
        LOCAL_FS.as_ref(),
    )
    .await;

    // Verify the patch contents - as otherwise we may have pulled contents
    // from the wrong file (as we're using relative paths)
    assert_eq!(
        result,
        MaybeApplyPatchVerified::Body(ApplyPatchAction {
            changes: HashMap::from([(
                session_dir.path().join(relative_path),
                ApplyPatchFileChange::Update {
                    unified_diff: r#"@@ -1 +1 @@
-session directory content
+updated session directory content
"#
                    .to_string(),
                    move_path: None,
                    new_content: "updated session directory content\n".to_string(),
                },
            )]),
            patch: argv[1].clone(),
            cwd: AbsolutePathBuf::from_absolute_path(session_dir.path()).unwrap(),
        })
    );
}

#[tokio::test]
async fn test_apply_patch_resolves_move_path_with_effective_cwd() {
    let session_dir = tempdir().unwrap();
    let worktree_rel = "alt";
    let worktree_dir = session_dir.path().join(worktree_rel);
    fs::create_dir_all(&worktree_dir).unwrap();

    let source_name = "old.txt";
    let dest_name = "renamed.txt";
    let source_path = worktree_dir.join(source_name);
    fs::write(&source_path, "before\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {source_name}
*** Move to: {dest_name}
@@
-before
+after"#
    ));

    let shell_script = format!("cd {worktree_rel} && apply_patch <<'PATCH'\n{patch}\nPATCH");
    let argv = vec!["bash".into(), "-lc".into(), shell_script];

    let result = maybe_parse_apply_patch_verified(
        &argv,
        &AbsolutePathBuf::from_absolute_path(session_dir.path()).unwrap(),
        LOCAL_FS.as_ref(),
    )
    .await;
    let action = match result {
        MaybeApplyPatchVerified::Body(action) => action,
        other => panic!("expected verified body, got {other:?}"),
    };

    assert_eq!(action.cwd.as_path(), worktree_dir.as_path());

    let change = action
        .changes()
        .get(&worktree_dir.join(source_name))
        .expect("source file change present");

    match change {
        ApplyPatchFileChange::Update { move_path, .. } => {
            assert_eq!(
                move_path.as_deref(),
                Some(worktree_dir.join(dest_name).as_path())
            );
        }
        other => panic!("expected update change, got {other:?}"),
    }
}
