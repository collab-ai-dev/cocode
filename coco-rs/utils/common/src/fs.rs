//! Race-safe regular-file reads and atomic file writes.

use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read as _;
use std::io::Write as _;
use std::path::Path;

/// Proof that an atomic replacement was reopened and matched byte-for-byte.
///
/// The field is private so callers cannot manufacture a successful commit.
#[derive(Debug)]
#[must_use]
pub struct VerifiedFileCommit(());

/// Open a path without ever blocking on a FIFO and validate the opened object.
///
/// Validation is descriptor-based: a path swap between lookup and open cannot
/// make the caller read from an unchecked object. On Unix, `O_NONBLOCK` makes
/// opening a FIFO safe; it has no effect on regular files.
pub fn open_regular(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_regular_open(&mut options);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path `{}` is not a regular file", path.display()),
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn configure_regular_open(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
}

#[cfg(not(unix))]
fn configure_regular_open(_options: &mut OpenOptions) {}

/// Read a regular file through a descriptor validated by [`open_regular`].
pub fn read_regular(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = open_regular(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Async regular-file read with the same descriptor validation semantics.
pub async fn read_regular_async(path: &Path) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    configure_regular_open_async(&mut options);
    let mut file = options.open(path).await?;
    if !file.metadata().await?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path `{}` is not a regular file", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(unix)]
fn configure_regular_open_async(options: &mut tokio::fs::OpenOptions) {
    options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
}

#[cfg(not(unix))]
fn configure_regular_open_async(_options: &mut tokio::fs::OpenOptions) {}

/// Write `contents` to `path` atomically — never leaves a truncated or
/// half-written file on disk. Writes through a sibling
/// [`tempfile::NamedTempFile`] in the target directory (a cross-filesystem
/// rename is a no-go) and `persist`s via atomic rename, creating parent
/// directories as needed. Readers (file watchers, concurrent loads) observe
/// either the old file or the complete new one.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_ref())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Atomically replace a regular file and verify the committed bytes.
///
/// Existing symlinks and non-regular targets are rejected. Existing mode bits
/// are copied to the replacement; hard links are intentionally broken by the
/// rename so writing through an alias cannot mutate another directory entry.
pub fn replace_regular_atomic(
    path: &Path,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<VerifiedFileCommit> {
    let contents = contents.as_ref();
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;

    let existing_permissions = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to replace symlink `{}`", path.display()),
            ));
        }
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path `{}` is not a regular file", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Match `OpenOptions::create` for a normal new file: the OS applies
        // the process umask to 0o666. NamedTempFile otherwise defaults to
        // 0o600, which would silently make newly written source files private.
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temp = builder.tempfile_in(parent)?;
    temp.write_all(contents)?;
    if let Some(permissions) = existing_permissions {
        temp.as_file().set_permissions(permissions)?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;

    #[cfg(unix)]
    File::open(parent)?.sync_all()?;

    verify_file_contents(path, contents)?;
    Ok(VerifiedFileCommit(()))
}

fn verify_file_contents(path: &Path, expected: &[u8]) -> std::io::Result<()> {
    let mut file = open_regular(path)?;
    if file.metadata()?.len() != expected.len() as u64 {
        return Err(verification_error(path));
    }

    let mut offset = 0;
    let mut buffer = [0u8; 16 * 1024];
    while offset < expected.len() {
        let read = file.read(&mut buffer)?;
        if read == 0
            || read > expected.len() - offset
            || buffer[..read] != expected[offset..offset + read]
        {
            return Err(verification_error(path));
        }
        offset += read;
    }
    let mut trailing = [0u8; 1];
    if file.read(&mut trailing)? != 0 {
        return Err(verification_error(path));
    }
    Ok(())
}

fn verification_error(path: &Path) -> std::io::Error {
    std::io::Error::other(format!(
        "atomic write verification failed for `{}`",
        path.display()
    ))
}

/// File modification time in epoch milliseconds.
///
/// Lives here rather than beside the read-state cache that consumes it:
/// `coco-types` holds types, and an async stat there would put `tokio::fs`
/// on every crate that depends on it.
pub async fn file_mtime_ms(path: &Path) -> std::io::Result<i64> {
    let meta = tokio::fs::metadata(path).await?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(mtime)
}

#[cfg(test)]
#[path = "fs.test.rs"]
mod tests;
