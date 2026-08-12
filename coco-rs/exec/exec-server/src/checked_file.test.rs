use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::io::Write as _;
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::Mode;

use super::platform::*;

fn open_original(path: &Path) -> io::Result<(OwnedFd, OsString, File)> {
    let (parent, name) = open_parent(path)?;
    let original = File::from(
        rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
            .map_err(io::Error::from)?,
    );
    Ok((parent, name, original))
}

fn named_stage(
    parent: &OwnedFd,
    contents: &[u8],
    preserve_metadata: Option<&File>,
) -> io::Result<StagedFile> {
    let (descriptor, name) = create_named_stage(parent)?;
    let mut staged = StagedFile {
        file: File::from(descriptor),
        name,
    };
    staged.file.write_all(contents)?;
    if let Some(original) = preserve_metadata {
        copy_security_metadata(original, &staged.file)?;
    }
    staged.file.sync_all()?;
    Ok(staged)
}

#[test]
fn named_stage_fallback_installs_a_missing_file_without_leaks() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("new.txt");
    let (parent, name) = open_parent(&path)?;
    let staged = named_stage(&parent, b"new", None)?;

    install_missing(&parent, &name, &staged)?;

    assert_eq!(std::fs::read_to_string(path)?, "new");
    assert_eq!(std::fs::read_dir(temp_dir.path())?.count(), 1);
    Ok(())
}

#[test]
fn named_stage_fallback_atomically_replaces_an_existing_file() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    std::fs::write(&path, "expected")?;
    let expected = super::snapshot(&path)?.expected;
    let (parent, name, original) = open_original(&path)?;
    let (transaction, transaction_name) = create_private_transaction(&parent)?;
    let staged = named_stage(&transaction, b"patched", Some(&original))?;

    replace_existing(
        &parent,
        &name,
        &original,
        &transaction,
        &transaction_name,
        &staged,
        &expected,
    )?;

    assert_eq!(std::fs::read_to_string(path)?, "patched");
    assert_eq!(std::fs::read_dir(temp_dir.path())?.count(), 1);
    Ok(())
}

#[test]
fn replacement_race_preserves_the_displaced_entry_without_rollback() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    std::fs::write(&path, "expected")?;
    let expected = super::snapshot(&path)?.expected;
    let (parent, name, original) = open_original(&path)?;
    let (transaction, transaction_name) = create_private_transaction(&parent)?;
    let staged = stage_private_file(&transaction, b"patched", Some(&original))?;

    std::fs::remove_file(&path)?;
    std::fs::write(&path, "concurrent")?;
    let error = replace_existing(
        &parent,
        &name,
        &original,
        &transaction,
        &transaction_name,
        &staged,
        &expected,
    )
    .expect_err("the atomic exchange must detect the displaced racer");

    assert!(error.to_string().contains("preserved"));
    assert_eq!(std::fs::read_to_string(&path)?, "patched");
    let recovered = temp_dir.path().join(transaction_name);
    assert_eq!(
        std::fs::read_to_string(recovered.join("displaced"))?,
        "concurrent"
    );
    Ok(())
}

#[test]
fn hard_link_race_is_detected_at_the_exchange_linearization_point() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    let alias = temp_dir.path().join("alias.txt");
    std::fs::write(&path, "expected")?;
    let expected = super::snapshot(&path)?.expected;
    let (parent, name, original) = open_original(&path)?;
    let (transaction, transaction_name) = create_private_transaction(&parent)?;
    let staged = stage_private_file(&transaction, b"patched", Some(&original))?;

    std::fs::hard_link(&path, &alias)?;
    let error = replace_existing(
        &parent,
        &name,
        &original,
        &transaction,
        &transaction_name,
        &staged,
        &expected,
    )
    .expect_err("the changed link count must fail the checked replacement");

    assert!(error.to_string().contains("preserved"));
    assert_eq!(std::fs::read_to_string(&path)?, "patched");
    assert_eq!(std::fs::read_to_string(&alias)?, "expected");
    Ok(())
}

#[test]
fn replacement_race_detects_security_metadata_only_changes() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    std::fs::write(&path, "expected")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    let expected = super::snapshot(&path)?.expected;
    let (parent, name, original) = open_original(&path)?;
    let (transaction, transaction_name) = create_private_transaction(&parent)?;
    let staged = stage_private_file(&transaction, b"patched", Some(&original))?;

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let error = replace_existing(
        &parent,
        &name,
        &original,
        &transaction,
        &transaction_name,
        &staged,
        &expected,
    )
    .expect_err("the captured security metadata must still match the snapshot");

    assert!(error.to_string().contains("preserved"));
    assert_eq!(std::fs::read_to_string(&path)?, "patched");
    let recovered = temp_dir.path().join(transaction_name).join("displaced");
    assert_eq!(
        std::fs::metadata(recovered)?.permissions().mode() & 0o7777,
        0o600
    );
    Ok(())
}

#[test]
fn removal_race_captures_the_new_entry_instead_of_unlinking_it() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("target.txt");
    std::fs::write(&path, "expected")?;
    let expected = super::snapshot(&path)?.expected;
    let (parent, name, original) = open_original(&path)?;

    std::fs::remove_file(&path)?;
    std::fs::write(&path, "concurrent")?;
    let error = capture_existing(&parent, &name, &original, &expected)
        .expect_err("the atomic capture must detect the displaced racer");

    assert!(error.to_string().contains("preserved"));
    assert!(!path.exists());
    let recovery = std::fs::read_dir(temp_dir.path())?
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".coco-checked-recovery-")
        })
        .expect("race must leave a recovery directory");
    assert_eq!(
        std::fs::read_to_string(recovery.path().join("displaced"))?,
        "concurrent"
    );
    Ok(())
}
