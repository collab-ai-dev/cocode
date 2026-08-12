use coco_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tokio::io;

use super::*;
use crate::is_file_mutation_conflict;

#[cfg(unix)]
use std::os::unix::fs::symlink;

#[tokio::test]
async fn direct_file_system_rejects_non_native_uri_as_invalid_input() {
    let error = DirectFileSystem
        .read_file(&non_native_uri(), /*sandbox*/ None)
        .await
        .expect_err("non-native URI should be rejected");

    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

fn non_native_uri() -> PathUri {
    #[cfg(unix)]
    let uri = "file://server/share/file.txt";
    #[cfg(windows)]
    let uri = "file:///usr/local/file.txt";

    match PathUri::parse(uri) {
        Ok(uri) => uri,
        Err(err) => panic!("valid non-native URI should parse: {err}"),
    }
}

#[cfg(unix)]
#[test]
fn resolve_existing_path_handles_symlink_parent_dotdot_escape() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let allowed_dir = temp_dir.path().join("allowed");
    let outside_dir = temp_dir.path().join("outside");
    std::fs::create_dir_all(&allowed_dir)?;
    std::fs::create_dir_all(&outside_dir)?;
    symlink(&outside_dir, allowed_dir.join("link"))?;

    let resolved = resolve_existing_path(
        allowed_dir
            .join("link")
            .join("..")
            .join("secret.txt")
            .as_path(),
    )?;

    assert_eq!(
        resolved,
        resolve_existing_path(temp_dir.path())?.join("secret.txt")
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn metadata_reports_dangling_symlink() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let link = temp_dir.path().join("dangling");
    symlink("missing", &link)?;
    let link = PathUri::from_path(&link)?;

    let metadata = LOCAL_FS.get_metadata(&link, None).await?;

    assert!(metadata.is_symlink);
    assert!(!metadata.is_file);
    assert!(!metadata.is_directory);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checked_write_rejects_a_stale_snapshot() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    std::fs::write(&path, "before")?;
    let path = PathUri::from_path(&path)?;
    let snapshot = LOCAL_FS.snapshot_file(&path, None).await?;
    std::fs::write(path.to_path_buf(), "external")?;

    let error = LOCAL_FS
        .write_file_checked(&path, b"patch".to_vec(), snapshot.expected, None)
        .await
        .expect_err("stale write must fail");

    assert!(is_file_mutation_conflict(&error));
    assert_eq!(std::fs::read_to_string(path.to_path_buf())?, "external");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checked_existing_write_atomically_replaces_the_inode_and_preserves_mode() -> io::Result<()>
{
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.sh");
    std::fs::write(&path, "before")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751))?;
    let before = std::fs::metadata(&path)?;
    let original = std::fs::File::open(&path)?;
    let before_inode_flags = rustix::fs::ioctl_getflags(&original)?;
    rustix::fs::fsetxattr(
        &original,
        "user.coco-test",
        b"must-survive",
        rustix::fs::XattrFlags::empty(),
    )?;
    let path = PathUri::from_path(&path)?;
    let snapshot = LOCAL_FS.snapshot_file(&path, None).await?;

    LOCAL_FS
        .write_file_checked(&path, b"after".to_vec(), snapshot.expected, None)
        .await?;

    let after = std::fs::metadata(path.to_path_buf())?;
    assert_ne!(
        after.ino(),
        before.ino(),
        "checked writes must not mutate in place"
    );
    assert_eq!(after.permissions().mode() & 0o7777, 0o751);
    assert_eq!(after.uid(), before.uid());
    assert_eq!(after.gid(), before.gid());
    let updated = std::fs::File::open(path.to_path_buf())?;
    assert_eq!(rustix::fs::ioctl_getflags(&updated)?, before_inode_flags);
    let mut xattr_value = vec![0; 64];
    let xattr_length = rustix::fs::fgetxattr(&updated, "user.coco-test", &mut xattr_value)?;
    xattr_value.truncate(xattr_length);
    assert_eq!(xattr_value, b"must-survive");
    assert_eq!(std::fs::read_to_string(path.to_path_buf())?, "after");
    assert!(
        std::fs::read_dir(temp_dir.path())?.all(|entry| !entry
            .map(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".coco-checked-recovery-"))
            .unwrap_or(false)),
        "successful transactions must remove their private recovery directory"
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn checked_mutations_fail_closed_without_atomic_platform_support() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let target = temp_dir.path().join("target.txt");
    std::fs::write(&target, "before")?;
    let target = PathUri::from_path(&target)?;
    #[cfg(not(unix))]
    {
        let snapshot_error = LOCAL_FS
            .snapshot_file(&target, None)
            .await
            .expect_err("checked snapshots must fail closed");
        assert_eq!(snapshot_error.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(unix)]
    assert!(matches!(
        LOCAL_FS.snapshot_file(&target, None).await?.expected,
        ExpectedFileState::File { .. }
    ));

    let write_error = LOCAL_FS
        .write_file_checked(&target, b"patch".to_vec(), ExpectedFileState::Missing, None)
        .await
        .expect_err("checked writes must fail closed");
    assert_eq!(write_error.kind(), io::ErrorKind::Unsupported);

    let remove_error = LOCAL_FS
        .remove_file_checked(&target, ExpectedFileState::Missing, None)
        .await
        .expect_err("checked removals must fail closed");
    assert_eq!(remove_error.kind(), io::ErrorKind::Unsupported);

    #[cfg(not(unix))]
    {
        let directory = PathUri::from_path(temp_dir.path().join("new-directory"))?;
        let directory_error = LOCAL_FS
            .create_directory_checked(&directory, None)
            .await
            .expect_err("checked directory creation must fail closed");
        assert_eq!(directory_error.kind(), io::ErrorKind::Unsupported);
    }
    assert_eq!(std::fs::read_to_string(target.to_path_buf())?, "before");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn checked_operations_reject_symlink_components() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let real = temp_dir.path().join("real");
    std::fs::create_dir(&real)?;
    std::fs::write(real.join("target.txt"), "secret")?;
    let linked = temp_dir.path().join("linked");
    symlink(&real, &linked)?;
    let target = PathUri::from_path(linked.join("target.txt"))?;

    LOCAL_FS
        .snapshot_file(&target, None)
        .await
        .expect_err("snapshot must not follow a symlink ancestor");
    assert_eq!(std::fs::read_to_string(real.join("target.txt"))?, "secret");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checked_existing_write_and_remove_reject_final_symlink_swap() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let target = temp_dir.path().join("target.txt");
    let protected = temp_dir.path().join("protected.txt");
    std::fs::write(&target, "before")?;
    std::fs::write(&protected, "protected")?;
    let target_uri = PathUri::from_path(&target)?;
    let expected = LOCAL_FS.snapshot_file(&target_uri, None).await?.expected;

    std::fs::remove_file(&target)?;
    symlink(&protected, &target)?;

    let write_error = LOCAL_FS
        .write_file_checked(&target_uri, b"attacker".to_vec(), expected.clone(), None)
        .await
        .expect_err("checked write must reject a swapped final symlink");
    assert!(is_file_mutation_conflict(&write_error));
    let remove_error = LOCAL_FS
        .remove_file_checked(&target_uri, expected, None)
        .await
        .expect_err("checked remove must reject a swapped final symlink");
    assert!(is_file_mutation_conflict(&remove_error));
    assert_eq!(std::fs::read_to_string(protected)?, "protected");
    assert!(std::fs::symlink_metadata(target)?.file_type().is_symlink());
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checked_missing_write_rejects_final_symlink_creation() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let target = temp_dir.path().join("target.txt");
    let protected = temp_dir.path().join("protected.txt");
    std::fs::write(&protected, "protected")?;
    let target_uri = PathUri::from_path(&target)?;
    let expected = LOCAL_FS.snapshot_file(&target_uri, None).await?.expected;
    assert!(matches!(expected, ExpectedFileState::Missing));

    symlink(&protected, &target)?;
    let error = LOCAL_FS
        .write_file_checked(&target_uri, b"attacker".to_vec(), expected, None)
        .await
        .expect_err("checked create must reject a swapped final symlink");

    assert!(is_file_mutation_conflict(&error));
    assert_eq!(std::fs::read_to_string(protected)?, "protected");
    assert!(std::fs::symlink_metadata(target)?.file_type().is_symlink());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_exposes_hard_link_count() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let first = temp_dir.path().join("first.txt");
    let second = temp_dir.path().join("second.txt");
    std::fs::write(&first, "shared")?;
    std::fs::hard_link(&first, &second)?;
    let snapshot = LOCAL_FS
        .snapshot_file(&PathUri::from_path(&first)?, None)
        .await?;

    let ExpectedFileState::File { version } = snapshot.expected else {
        panic!("existing file snapshot");
    };
    assert_eq!(version.link_count, 2);
    Ok(())
}

#[cfg(windows)]
#[test]
fn symlink_points_to_directory_handles_dangling_directory_symlinks() -> io::Result<()> {
    use std::os::windows::fs::symlink_dir;

    let temp_dir = tempfile::TempDir::new()?;
    let source_dir = temp_dir.path().join("source");
    let link_path = temp_dir.path().join("source-link");
    std::fs::create_dir(&source_dir)?;

    if symlink_dir(&source_dir, &link_path).is_err() {
        return Ok(());
    }

    std::fs::remove_dir(&source_dir)?;

    assert!(symlink_points_to_directory(&link_path)?);
    Ok(())
}
