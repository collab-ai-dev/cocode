#[cfg(unix)]
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::io::Seek;
#[cfg(unix)]
use std::io::SeekFrom;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use sha2::Digest as _;

use crate::ExpectedFileState;
use crate::FileSnapshot;
#[cfg(unix)]
use crate::FileVersion;
#[cfg(unix)]
use crate::file_mutation_conflict;

#[cfg(unix)]
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn snapshot(path: &Path) -> io::Result<FileSnapshot> {
    platform::snapshot(path)
}

pub(crate) fn write_checked(
    path: &Path,
    contents: &[u8],
    expected: &ExpectedFileState,
) -> io::Result<()> {
    platform::write_checked(path, contents, expected)
}

pub(crate) fn remove_checked(path: &Path, expected: &ExpectedFileState) -> io::Result<()> {
    platform::remove_checked(path, expected)
}

pub(crate) fn create_directory_checked(path: &Path) -> io::Result<()> {
    platform::create_directory_checked(path)
}

#[cfg(unix)]
fn snapshot_open_file(file: &mut File) -> io::Result<FileSnapshot> {
    let before = file.metadata()?;
    let before_security_metadata_sha256 = platform::security_metadata_sha256(file)?;
    if !before.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "checked filesystem operations require a regular file",
        ));
    }
    if before.len() > MAX_SNAPSHOT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file is too large to snapshot: limit is {MAX_SNAPSHOT_BYTES} bytes"),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut contents = Vec::new();
    (&mut *file)
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file is too large to snapshot: limit is {MAX_SNAPSHOT_BYTES} bytes"),
        ));
    }
    let after = file.metadata()?;
    let after_security_metadata_sha256 = platform::security_metadata_sha256(file)?;
    let before_version = version_for_metadata(&before, &contents, before_security_metadata_sha256);
    let after_version = version_for_metadata(&after, &contents, after_security_metadata_sha256);
    if before_version.identity != after_version.identity
        || before_version.size != after_version.size
        || before_version.link_count != after_version.link_count
        || before_version.security_metadata_sha256 != after_version.security_metadata_sha256
    {
        return Err(file_mutation_conflict());
    }
    Ok(FileSnapshot::file(after_version, contents))
}

#[cfg(unix)]
fn ensure_expected(actual: &FileSnapshot, expected: &ExpectedFileState) -> io::Result<()> {
    if &actual.expected == expected {
        Ok(())
    } else {
        Err(file_mutation_conflict())
    }
}

#[cfg(unix)]
fn version_for_metadata(
    metadata: &std::fs::Metadata,
    contents: &[u8],
    security_metadata_sha256: String,
) -> FileVersion {
    FileVersion {
        identity: platform::metadata_identity(metadata),
        content_sha256: format!("{:x}", sha2::Sha256::digest(contents)),
        security_metadata_sha256,
        size: metadata.len(),
        link_count: platform::metadata_link_count(metadata),
    }
}

#[cfg(unix)]
mod platform {
    use std::ffi::OsString;
    use std::os::unix::fs::MetadataExt as _;
    #[cfg(target_os = "linux")]
    use std::os::unix::io::AsRawFd as _;
    use std::path::Component;

    use rustix::fd::OwnedFd;
    use rustix::fs::AtFlags;
    use rustix::fs::Mode;
    use rustix::fs::OFlags;
    use sha2::Digest as _;

    use super::*;

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const FILE_READ_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const FILE_WRITE_FLAGS: OFlags = OFlags::RDWR.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

    pub(super) fn snapshot(path: &Path) -> io::Result<FileSnapshot> {
        let (parent, name) = match open_parent(path) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FileSnapshot::missing());
            }
            Err(error) => return Err(error),
        };
        let descriptor = match rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty()) {
            Ok(descriptor) => descriptor,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(FileSnapshot::missing()),
            Err(error) => return Err(error.into()),
        };
        let mut file = File::from(descriptor);
        snapshot_open_file(&mut file)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn write_checked(
        path: &Path,
        contents: &[u8],
        expected: &ExpectedFileState,
    ) -> io::Result<()> {
        let (parent, name) = open_parent(path)?;
        match expected {
            ExpectedFileState::Missing => {
                let staged = stage_private_file(&parent, contents, None)?;
                install_missing(&parent, &name, &staged)
            }
            ExpectedFileState::File { .. } => {
                let descriptor = rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
                    .map_err(|error| {
                        if matches!(error, rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) {
                            file_mutation_conflict()
                        } else {
                            error.into()
                        }
                    })?;
                let mut file = File::from(descriptor);
                let actual = snapshot_open_file(&mut file)?;
                ensure_expected(&actual, expected)?;
                ensure_directory_entry_matches(&parent, &name, &file)?;
                let (transaction, transaction_name) = create_private_transaction(&parent)?;
                let staged = match stage_private_file(&transaction, contents, Some(&file)) {
                    Ok(staged) => staged,
                    Err(error) => {
                        let _ =
                            remove_private_transaction(&parent, &transaction_name, &transaction);
                        return Err(error);
                    }
                };

                // Recheck after potentially slow staging. The old inode is
                // never modified: even a hard link created after this check
                // continues to reference its unchanged contents.
                let actual = match snapshot_open_file(&mut file) {
                    Ok(actual) => actual,
                    Err(error) => {
                        let _ =
                            remove_private_transaction(&parent, &transaction_name, &transaction);
                        return Err(error);
                    }
                };
                if let Err(error) = ensure_expected(&actual, expected)
                    .and_then(|()| ensure_directory_entry_matches(&parent, &name, &file))
                {
                    let _ = remove_private_transaction(&parent, &transaction_name, &transaction);
                    return Err(error);
                }
                replace_existing(
                    &parent,
                    &name,
                    &file,
                    &transaction,
                    &transaction_name,
                    &staged,
                    expected,
                )
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn write_checked(
        _path: &Path,
        _contents: &[u8],
        _expected: &ExpectedFileState,
    ) -> io::Result<()> {
        Err(super::checked_mutation_unsupported())
    }

    #[cfg(target_os = "linux")]
    fn stage_private_file(
        parent: &OwnedFd,
        contents: &[u8],
        preserve_metadata: Option<&File>,
    ) -> io::Result<File> {
        let flags = FILE_WRITE_FLAGS.union(OFlags::TMPFILE);
        let descriptor = rustix::fs::openat(parent, ".", flags, Mode::from_raw_mode(0o666))
            .map_err(io::Error::from)?;
        let mut file = File::from(descriptor);
        file.write_all(contents)?;
        if let Some(original) = preserve_metadata {
            copy_security_metadata(original, &file)?;
        }
        file.sync_all()?;
        Ok(file)
    }

    #[cfg(target_os = "linux")]
    fn copy_security_metadata(original: &File, staged: &File) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;

        let metadata = original.metadata()?;
        let inode_flags = rustix::fs::ioctl_getflags(original).map_err(io::Error::from)?;
        // O_TMPFILE creation may inherit an access ACL from the private
        // transaction directory. Remove all inherited attributes so absence
        // on the source is preserved just as faithfully as presence.
        for name in read_xattr_names(staged)? {
            rustix::fs::fremovexattr(staged, std::ffi::OsStr::from_bytes(&name))
                .map_err(io::Error::from)?;
        }
        rustix::fs::fchown(
            staged,
            Some(rustix::fs::Uid::from_raw(metadata.uid())),
            Some(rustix::fs::Gid::from_raw(metadata.gid())),
        )
        .map_err(io::Error::from)?;

        for name in read_xattr_names(original)? {
            let value = read_xattr(original, &name)?;
            rustix::fs::fsetxattr(
                staged,
                std::ffi::OsStr::from_bytes(&name),
                &value,
                rustix::fs::XattrFlags::empty(),
            )
            .map_err(io::Error::from)?;
        }

        // chown and ACL writes may clear or adjust mode bits. Apply the
        // source mode last so the published inode has the same effective
        // permission mask, set-id bits, and sticky bit as the source.
        rustix::fs::fchmod(staged, Mode::from_raw_mode(metadata.mode() & 0o7777))
            .map_err(io::Error::from)?;
        rustix::fs::ioctl_setflags(staged, inode_flags).map_err(io::Error::from)?;

        if security_metadata_sha256(original)? == security_metadata_sha256(staged)? {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    #[cfg(target_os = "linux")]
    fn read_xattr_names(file: &File) -> io::Result<Vec<Vec<u8>>> {
        let bytes = read_dynamic_xattr_buffer(|buffer| rustix::fs::flistxattr(file, buffer))?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        if bytes.last() != Some(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "extended-attribute name list was not NUL terminated",
            ));
        }
        bytes[..bytes.len() - 1]
            .split(|byte| *byte == 0)
            .map(|name| {
                if name.is_empty() {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "extended-attribute name must not be empty",
                    ))
                } else {
                    Ok(name.to_vec())
                }
            })
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn read_xattr(file: &File, name: &[u8]) -> io::Result<Vec<u8>> {
        use std::os::unix::ffi::OsStrExt as _;

        read_dynamic_xattr_buffer(|buffer| {
            rustix::fs::fgetxattr(file, std::ffi::OsStr::from_bytes(name), buffer)
        })
    }

    #[cfg(target_os = "linux")]
    fn read_dynamic_xattr_buffer(
        mut read: impl FnMut(&mut Vec<u8>) -> Result<usize, rustix::io::Errno>,
    ) -> io::Result<Vec<u8>> {
        const INITIAL_CAPACITY: usize = 256;
        const MAX_CAPACITY: usize = 1024 * 1024;

        let mut capacity = INITIAL_CAPACITY;
        loop {
            let mut value = vec![0; capacity];
            match read(&mut value) {
                Ok(length) => {
                    value.truncate(length);
                    return Ok(value);
                }
                Err(rustix::io::Errno::RANGE) if capacity < MAX_CAPACITY => {
                    capacity = (capacity * 2).min(MAX_CAPACITY);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn install_missing(parent: &OwnedFd, name: &OsString, staged: &File) -> io::Result<()> {
        link_staged(parent, name, staged).map_err(|error| {
            if matches!(error, rustix::io::Errno::EXIST | rustix::io::Errno::LOOP) {
                file_mutation_conflict()
            } else {
                error.into()
            }
        })?;
        ensure_directory_entry_matches(parent, name, staged)
    }

    #[cfg(target_os = "linux")]
    fn replace_existing(
        parent: &OwnedFd,
        name: &OsString,
        original: &File,
        transaction: &OwnedFd,
        transaction_name: &OsString,
        staged: &File,
        expected: &ExpectedFileState,
    ) -> io::Result<()> {
        let entry = OsString::from("displaced");
        link_staged(transaction, &entry, staged).map_err(io::Error::from)?;
        if let Err(error) = rustix::fs::renameat_with(
            transaction,
            &entry,
            parent,
            name,
            rustix::fs::RenameFlags::EXCHANGE,
        ) {
            let _ = rustix::fs::unlinkat(transaction, &entry, AtFlags::empty());
            let _ = remove_private_transaction(parent, transaction_name, transaction);
            return Err(if error == rustix::io::Errno::NOENT {
                file_mutation_conflict()
            } else {
                error.into()
            });
        }

        // EXCHANGE makes the old target available inside a private 0700
        // transaction directory, so comparison and replacement have one
        // atomic linearization point. A mismatching racer is preserved there:
        // rollback would itself race and could overwrite a newer target.
        let displaced_matches = ensure_directory_entry_matches(transaction, &entry, original)
            .and_then(|()| snapshot_named(transaction, &entry))
            .and_then(|actual| ensure_expected_contents(&actual, expected))
            .is_ok();
        if !displaced_matches {
            return Err(io::Error::other(format!(
                "checked replacement raced with another filesystem mutation; the displaced entry is preserved in adjacent recovery directory {} and the target state is unknown",
                transaction_name.to_string_lossy()
            )));
        }
        rustix::fs::unlinkat(transaction, &entry, AtFlags::empty()).map_err(io::Error::from)?;
        remove_private_transaction(parent, transaction_name, transaction)
    }

    #[cfg(target_os = "linux")]
    fn snapshot_named(parent: &OwnedFd, name: &OsString) -> io::Result<FileSnapshot> {
        let descriptor = rustix::fs::openat(parent, name, FILE_READ_FLAGS, Mode::empty())
            .map_err(io::Error::from)?;
        let mut file = File::from(descriptor);
        snapshot_open_file(&mut file)
    }

    #[cfg(target_os = "linux")]
    fn ensure_expected_contents(
        actual: &FileSnapshot,
        expected: &ExpectedFileState,
    ) -> io::Result<()> {
        let (
            ExpectedFileState::File {
                version: actual_version,
            },
            ExpectedFileState::File {
                version: expected_version,
            },
        ) = (&actual.expected, expected)
        else {
            return Err(file_mutation_conflict());
        };
        if actual_version.content_sha256 == expected_version.content_sha256
            && actual_version.security_metadata_sha256 == expected_version.security_metadata_sha256
            && actual_version.size == expected_version.size
            && actual_version.link_count == expected_version.link_count
        {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    #[cfg(target_os = "linux")]
    fn link_staged(
        parent: &OwnedFd,
        name: &OsString,
        staged: &File,
    ) -> Result<(), rustix::io::Errno> {
        // Some container kernels reject AT_EMPTY_PATH for unprivileged users
        // even for O_TMPFILE descriptors. `/proc/self/fd` names only this
        // process's already-open private inode and is the documented linkat
        // fallback; SYMLINK_FOLLOW links the inode, not the procfs symlink.
        let source = format!("/proc/self/fd/{}", staged.as_raw_fd());
        rustix::fs::linkat(
            rustix::fs::CWD,
            source,
            parent,
            name,
            AtFlags::SYMLINK_FOLLOW,
        )
    }

    #[cfg(target_os = "linux")]
    pub(super) fn remove_checked(path: &Path, expected: &ExpectedFileState) -> io::Result<()> {
        let ExpectedFileState::File { .. } = expected else {
            return Err(file_mutation_conflict());
        };
        let (parent, name) = open_parent(path)?;
        let descriptor = rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
            .map_err(|error| {
                if matches!(error, rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) {
                    file_mutation_conflict()
                } else {
                    error.into()
                }
            })?;
        let mut file = File::from(descriptor);
        let actual = snapshot_open_file(&mut file)?;
        ensure_expected(&actual, expected)?;

        capture_existing(&parent, &name, &file, expected)
    }

    #[cfg(target_os = "linux")]
    fn capture_existing(
        parent: &OwnedFd,
        name: &OsString,
        original: &File,
        expected: &ExpectedFileState,
    ) -> io::Result<()> {
        let (transaction, transaction_name) = create_private_transaction(parent)?;
        let entry = OsString::from("displaced");
        if let Err(error) = rustix::fs::renameat_with(
            parent,
            name,
            &transaction,
            &entry,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            let _ = remove_private_transaction(parent, &transaction_name, &transaction);
            return Err(
                if matches!(error, rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) {
                    file_mutation_conflict()
                } else {
                    error.into()
                },
            );
        }

        let displaced_matches = ensure_directory_entry_matches(&transaction, &entry, original)
            .and_then(|()| snapshot_named(&transaction, &entry))
            .and_then(|actual| ensure_expected_contents(&actual, expected))
            .is_ok();
        if !displaced_matches {
            return Err(io::Error::other(format!(
                "checked removal raced with another filesystem mutation; the captured entry is preserved in adjacent recovery directory {} and the target state is unknown",
                transaction_name.to_string_lossy()
            )));
        }

        rustix::fs::unlinkat(&transaction, &entry, AtFlags::empty()).map_err(io::Error::from)?;
        remove_private_transaction(parent, &transaction_name, &transaction)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn remove_checked(_path: &Path, _expected: &ExpectedFileState) -> io::Result<()> {
        Err(super::checked_mutation_unsupported())
    }

    #[cfg(target_os = "linux")]
    fn create_private_transaction(parent: &OwnedFd) -> io::Result<(OwnedFd, OsString)> {
        let name = OsString::from(format!(
            ".coco-checked-recovery-{}",
            uuid::Uuid::new_v4().simple()
        ));
        rustix::fs::mkdirat(parent, &name, Mode::from_raw_mode(0o700)).map_err(io::Error::from)?;
        match rustix::fs::openat(parent, &name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(directory) => Ok((directory, name)),
            Err(error) => {
                let _ = rustix::fs::unlinkat(parent, &name, AtFlags::REMOVEDIR);
                Err(error.into())
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn remove_private_transaction(
        parent: &OwnedFd,
        name: &OsString,
        transaction: &OwnedFd,
    ) -> io::Result<()> {
        ensure_directory_entry_matches(parent, name, transaction)?;
        rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)
    }

    pub(super) fn create_directory_checked(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path)?;
        rustix::fs::mkdirat(&parent, &name, Mode::from_raw_mode(0o777)).map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                file_mutation_conflict()
            } else {
                error.into()
            }
        })
    }

    pub(super) fn metadata_identity(metadata: &std::fs::Metadata) -> String {
        format!(
            "unix:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }

    pub(super) fn metadata_link_count(metadata: &std::fs::Metadata) -> u64 {
        metadata.nlink()
    }

    #[cfg(target_os = "linux")]
    pub(super) fn security_metadata_sha256(file: &File) -> io::Result<String> {
        let metadata = file.metadata()?;
        let inode_flags = rustix::fs::ioctl_getflags(file).map_err(io::Error::from)?;
        let mut attributes = read_xattr_names(file)?
            .into_iter()
            .map(|name| {
                let value = read_xattr(file, &name)?;
                Ok((name, value))
            })
            .collect::<io::Result<Vec<_>>>()?;
        attributes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let mut canonical = Vec::new();
        canonical.extend_from_slice(b"coco-linux-security-metadata-v1\0");
        canonical.extend_from_slice(&metadata.uid().to_le_bytes());
        canonical.extend_from_slice(&metadata.gid().to_le_bytes());
        canonical.extend_from_slice(&(metadata.mode() & 0o7777).to_le_bytes());
        canonical.extend_from_slice(&inode_flags.bits().to_le_bytes());
        for (name, value) in attributes {
            append_length_prefixed(&mut canonical, &name);
            append_length_prefixed(&mut canonical, &value);
        }
        Ok(format!("{:x}", sha2::Sha256::digest(canonical)))
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn security_metadata_sha256(file: &File) -> io::Result<String> {
        let metadata = file.metadata()?;
        let mut canonical = Vec::new();
        canonical.extend_from_slice(b"coco-unix-security-metadata-v1\0");
        canonical.extend_from_slice(&metadata.uid().to_le_bytes());
        canonical.extend_from_slice(&metadata.gid().to_le_bytes());
        canonical.extend_from_slice(&(metadata.mode() & 0o7777).to_le_bytes());
        Ok(format!("{:x}", sha2::Sha256::digest(canonical)))
    }

    #[cfg(target_os = "linux")]
    fn append_length_prefixed(target: &mut Vec<u8>, value: &[u8]) {
        target.extend_from_slice(&(value.len() as u64).to_le_bytes());
        target.extend_from_slice(value);
    }

    fn open_parent(path: &Path) -> io::Result<(OwnedFd, OsString)> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checked filesystem path must be absolute",
            ));
        }
        let mut components = path.components().peekable();
        if !matches!(components.next(), Some(Component::RootDir)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checked filesystem path must start at the filesystem root",
            ));
        }
        let mut normal = Vec::new();
        for component in components {
            match component {
                Component::Normal(segment) => normal.push(segment.to_os_string()),
                Component::CurDir => {}
                Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "checked filesystem path must be normalized",
                    ));
                }
            }
        }
        let name = normal.pop().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "cannot mutate filesystem root")
        })?;
        let mut directory =
            rustix::fs::open("/", DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        for segment in normal {
            directory = rustix::fs::openat(&directory, &segment, DIRECTORY_FLAGS, Mode::empty())
                .map_err(io::Error::from)?;
        }
        Ok((directory, name))
    }

    fn ensure_directory_entry_matches(
        parent: &OwnedFd,
        name: &OsString,
        file: &impl std::os::fd::AsFd,
    ) -> io::Result<()> {
        let entry =
            rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
                if matches!(error, rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) {
                    file_mutation_conflict()
                } else {
                    error.into()
                }
            })?;
        let opened = rustix::fs::fstat(file).map_err(io::Error::from)?;
        if entry.st_dev == opened.st_dev && entry.st_ino == opened.st_ino {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        use super::*;

        #[test]
        fn replacement_race_preserves_the_displaced_entry_without_rollback() -> io::Result<()> {
            let temp_dir = tempfile::TempDir::new()?;
            let path = temp_dir.path().join("target.txt");
            std::fs::write(&path, "expected")?;
            let expected = super::snapshot(&path)?.expected;
            let (parent, name) = open_parent(&path)?;
            let original = File::from(
                rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
                    .map_err(io::Error::from)?,
            );
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
            assert_eq!(
                std::fs::read_to_string(temp_dir.path().join(transaction_name).join("displaced"))?,
                "concurrent"
            );
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
            let (parent, name) = open_parent(&path)?;
            let original = File::from(
                rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
                    .map_err(io::Error::from)?,
            );
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
            let (parent, name) = open_parent(&path)?;
            let original = File::from(
                rustix::fs::openat(&parent, &name, FILE_READ_FLAGS, Mode::empty())
                    .map_err(io::Error::from)?,
            );

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
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;

    pub(super) fn snapshot(_path: &Path) -> io::Result<FileSnapshot> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-follow filesystem snapshots are unsupported on this platform",
        ))
    }

    pub(super) fn write_checked(
        _path: &Path,
        _contents: &[u8],
        _expected: &ExpectedFileState,
    ) -> io::Result<()> {
        Err(checked_mutation_unsupported())
    }

    pub(super) fn remove_checked(_path: &Path, _expected: &ExpectedFileState) -> io::Result<()> {
        Err(checked_mutation_unsupported())
    }

    pub(super) fn create_directory_checked(_path: &Path) -> io::Result<()> {
        Err(checked_mutation_unsupported())
    }
}

#[cfg(not(target_os = "linux"))]
fn checked_mutation_unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic checked filesystem mutations are unsupported on this platform",
    )
}
