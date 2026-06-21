use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn create_with_absolute_path_ignores_base_path() {
    let base_dir = tempdir().expect("base dir");
    let absolute_dir = tempdir().expect("absolute dir");
    let base_path = base_dir.path();
    let absolute_path = absolute_dir.path().join("file.txt");
    let abs_path_buf = AbsolutePathBuf::resolve_path_against_base(absolute_path.clone(), base_path);
    assert_eq!(abs_path_buf.as_path(), absolute_path.as_path());
}

#[test]
fn relative_path_is_resolved_against_base_path() {
    let temp_dir = tempdir().expect("base dir");
    let base_dir = temp_dir.path();
    let abs_path_buf = AbsolutePathBuf::resolve_path_against_base("file.txt", base_dir);
    assert_eq!(abs_path_buf.as_path(), base_dir.join("file.txt").as_path());
}

#[test]
fn relative_path_dots_are_normalized_against_base_path() {
    let temp_dir = tempdir().expect("base dir");
    let base_dir = temp_dir.path();
    let abs_path_buf = AbsolutePathBuf::resolve_path_against_base("./nested/../file.txt", base_dir);
    assert_eq!(abs_path_buf.as_path(), base_dir.join("file.txt").as_path());
}

#[test]
fn relative_to_current_dir_resolves_relative_path() -> std::io::Result<()> {
    let current_dir = std::env::current_dir()?;
    let abs_path_buf = AbsolutePathBuf::relative_to_current_dir("file.txt")?;
    assert_eq!(
        abs_path_buf.as_path(),
        current_dir.join("file.txt").as_path()
    );
    Ok(())
}

#[test]
fn guard_used_in_deserialization() {
    let temp_dir = tempdir().expect("base dir");
    let base_dir = temp_dir.path();
    let relative_path = "subdir/file.txt";
    let abs_path_buf = {
        let _guard = AbsolutePathBufGuard::new(base_dir);
        serde_json::from_str::<AbsolutePathBuf>(&format!(r#""{relative_path}""#))
            .expect("failed to deserialize")
    };
    assert_eq!(
        abs_path_buf.as_path(),
        base_dir.join(relative_path).as_path()
    );
}

#[test]
fn home_directory_root_is_expanded_in_deserialization() {
    let Some(home) = home_dir() else {
        return;
    };
    let temp_dir = tempdir().expect("base dir");
    let abs_path_buf = {
        let _guard = AbsolutePathBufGuard::new(temp_dir.path());
        serde_json::from_str::<AbsolutePathBuf>("\"~\"").expect("failed to deserialize")
    };
    assert_eq!(abs_path_buf.as_path(), home.as_path());
}

#[test]
fn home_directory_subpath_is_expanded_in_deserialization() {
    let Some(home) = home_dir() else {
        return;
    };
    let temp_dir = tempdir().expect("base dir");
    let abs_path_buf = {
        let _guard = AbsolutePathBufGuard::new(temp_dir.path());
        serde_json::from_str::<AbsolutePathBuf>("\"~/code\"").expect("failed to deserialize")
    };
    assert_eq!(abs_path_buf.as_path(), home.join("code").as_path());
}

#[test]
fn home_directory_double_slash_is_expanded_in_deserialization() {
    let Some(home) = home_dir() else {
        return;
    };
    let temp_dir = tempdir().expect("base dir");
    let abs_path_buf = {
        let _guard = AbsolutePathBufGuard::new(temp_dir.path());
        serde_json::from_str::<AbsolutePathBuf>("\"~//code\"").expect("failed to deserialize")
    };
    assert_eq!(abs_path_buf.as_path(), home.join("code").as_path());
}

#[cfg(unix)]
#[test]
fn canonicalize_preserving_symlinks_keeps_logical_symlink_path() {
    let temp_dir = tempdir().expect("temp dir");
    let real = temp_dir.path().join("real");
    let link = temp_dir.path().join("link");
    std::fs::create_dir_all(&real).expect("create real dir");
    std::os::unix::fs::symlink(&real, &link).expect("create symlink");

    let canonicalized =
        canonicalize_preserving_symlinks(&link).expect("canonicalize preserving symlinks");

    assert_eq!(canonicalized, link);
}

#[cfg(unix)]
#[test]
fn canonicalize_preserving_symlinks_keeps_logical_missing_child_under_symlink() {
    let temp_dir = tempdir().expect("temp dir");
    let real = temp_dir.path().join("real");
    let link = temp_dir.path().join("link");
    std::fs::create_dir_all(&real).expect("create real dir");
    std::os::unix::fs::symlink(&real, &link).expect("create symlink");
    let missing = link.join("missing.txt");

    let canonicalized =
        canonicalize_preserving_symlinks(&missing).expect("canonicalize preserving symlinks");

    assert_eq!(canonicalized, missing);
}

#[test]
fn canonicalize_existing_preserving_symlinks_errors_for_missing_path() {
    let temp_dir = tempdir().expect("temp dir");
    let missing = temp_dir.path().join("missing");

    let err = canonicalize_existing_preserving_symlinks(&missing)
        .expect_err("missing path should fail canonicalization");

    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn canonicalize_existing_preserving_symlinks_keeps_logical_symlink_path() {
    let temp_dir = tempdir().expect("temp dir");
    let real = temp_dir.path().join("real");
    let link = temp_dir.path().join("link");
    std::fs::create_dir_all(&real).expect("create real dir");
    std::os::unix::fs::symlink(&real, &link).expect("create symlink");

    let canonicalized =
        canonicalize_existing_preserving_symlinks(&link).expect("canonicalize symlink");

    assert_eq!(canonicalized, link);
}

#[cfg(target_os = "windows")]
#[test]
fn home_directory_backslash_subpath_is_expanded_in_deserialization() {
    let Some(home) = home_dir() else {
        return;
    };
    let temp_dir = tempdir().expect("base dir");
    let abs_path_buf = {
        let _guard = AbsolutePathBufGuard::new(temp_dir.path());
        let input = serde_json::to_string(r#"~\code"#).expect("string should serialize as JSON");
        serde_json::from_str::<AbsolutePathBuf>(&input).expect("is valid abs path")
    };
    assert_eq!(abs_path_buf.as_path(), home.join("code").as_path());
}

#[cfg(target_os = "windows")]
#[test]
fn canonicalize_preserving_symlinks_avoids_verbatim_prefixes() {
    let temp_dir = tempdir().expect("temp dir");

    let canonicalized = canonicalize_preserving_symlinks(temp_dir.path()).expect("canonicalize");

    assert_eq!(
        canonicalized,
        dunce::canonicalize(temp_dir.path()).expect("canonicalize temp dir")
    );
    assert!(
        !canonicalized.to_string_lossy().starts_with(r"\\?\"),
        "expected a non-verbatim Windows path, got {canonicalized:?}"
    );
}
