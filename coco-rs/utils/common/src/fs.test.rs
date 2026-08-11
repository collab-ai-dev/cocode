use super::*;

#[test]
fn atomic_replace_preserves_mode_and_breaks_hard_links() {
    let temp = tempfile::tempdir().unwrap();
    let protected = temp.path().join("AGENTS.md");
    let alias = temp.path().join("ordinary.md");
    std::fs::write(&protected, b"protected").unwrap();
    std::fs::hard_link(&protected, &alias).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&alias, std::fs::Permissions::from_mode(0o640)).unwrap();
    }

    let _verified = replace_regular_atomic(&alias, b"replacement").unwrap();

    assert_eq!(std::fs::read(&protected).unwrap(), b"protected");
    assert_eq!(std::fs::read(&alias).unwrap(), b"replacement");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&alias).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}

#[cfg(unix)]
#[test]
fn atomic_create_respects_the_process_umask_like_a_normal_file() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().unwrap();
    let control = temp.path().join("control");
    let committed = temp.path().join("committed");
    std::fs::write(&control, b"control").unwrap();

    let _verified = replace_regular_atomic(&committed, b"committed").unwrap();

    let control_mode = std::fs::metadata(control).unwrap().permissions().mode() & 0o777;
    let committed_mode = std::fs::metadata(committed).unwrap().permissions().mode() & 0o777;
    assert_eq!(committed_mode, control_mode);
}

#[cfg(unix)]
#[test]
fn regular_open_rejects_fifo_without_blocking() {
    let temp = tempfile::tempdir().unwrap();
    let fifo = temp.path().join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());

    let error = open_regular(&fifo).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn atomic_replace_rejects_symlink_targets() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    std::fs::write(&target, b"original").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let error = replace_regular_atomic(&link, b"replacement").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
}
