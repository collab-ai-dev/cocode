use bytes::Bytes;
use coco_utils_path_uri::PathUri;
use futures::Stream;
use serde::Deserialize;
use serde::Serialize;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

/// Maximum chunk size returned by [`ExecutorFileSystem::read_file_stream`].
pub const FILE_READ_CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateDirectoryOptions {
    pub recursive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOptions {
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub is_directory: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    /// Size in bytes.
    pub size: u64,
    pub created_at_ms: i64,
    pub modified_at_ms: i64,
}

/// Opaque identity and content token returned by an executor-owned snapshot.
/// Callers pass the complete value back unchanged to checked mutations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub identity: String,
    pub content_sha256: String,
    /// Digest of ownership, mode, ACLs/xattrs, security labels, capabilities,
    /// and platform inode flags captured by the executor.
    pub security_metadata_sha256: String,
    pub size: u64,
    pub link_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ExpectedFileState {
    Missing,
    File { version: FileVersion },
}

/// Contents and version captured from one no-follow file handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSnapshot {
    pub expected: ExpectedFileState,
    pub contents: Option<Vec<u8>>,
}

impl FileSnapshot {
    pub fn missing() -> Self {
        Self {
            expected: ExpectedFileState::Missing,
            contents: None,
        }
    }

    pub fn file(version: FileVersion, contents: Vec<u8>) -> Self {
        Self {
            expected: ExpectedFileState::File { version },
            contents: Some(contents),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("filesystem target changed after it was prepared")]
pub struct FileMutationConflict;

pub fn file_mutation_conflict() -> io::Error {
    io::Error::other(FileMutationConflict)
}

pub fn is_file_mutation_conflict(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<FileMutationConflict>)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadDirectoryEntry {
    pub file_name: String,
    pub is_directory: bool,
    pub is_file: bool,
}

/// Serialized sandbox intent carried over the exec-server protocol.
///
/// coco exec-server v1 does not implement upstream sandbox helpers. The server
/// preserves this protocol field for compatibility and rejects sandboxed
/// filesystem/process requests explicitly at the execution boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemSandboxContext {
    #[serde(flatten)]
    pub value: serde_json::Value,
}

impl FileSystemSandboxContext {
    pub fn should_run_in_sandbox(&self) -> bool {
        true
    }
}

pub type FileSystemResult<T> = io::Result<T>;

pub type ExecutorFileSystemFuture<'a, T> =
    Pin<Box<dyn Future<Output = FileSystemResult<T>> + Send + 'a>>;

pub struct FileSystemReadStream {
    inner: Pin<Box<dyn Stream<Item = FileSystemResult<Bytes>> + Send + 'static>>,
}

impl FileSystemReadStream {
    pub fn new(stream: impl Stream<Item = FileSystemResult<Bytes>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl Stream for FileSystemReadStream {
    type Item = FileSystemResult<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

pub trait ExecutorFileSystem: Send + Sync {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri>;

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>>;

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream>;

    fn read_file_text<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, String> {
        Box::pin(async move {
            let bytes = self.read_file(path, sandbox).await?;
            String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
        })
    }

    /// Snapshot a missing or regular file without following symbolic-link
    /// components. The returned version is meaningful only to this executor.
    fn snapshot_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSnapshot>;

    /// Write only when the target still matches `expected`.
    ///
    /// Implementations must not follow symbolic-link components or mutate an
    /// already-open inode in place. A failure after the commit linearization
    /// point must preserve displaced data and report the resulting state as
    /// unknown rather than attempting a check-then-act rollback.
    fn write_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    /// Remove only when the target still matches `expected`.
    ///
    /// Implementations must atomically capture the directory entry before
    /// validating it. If the captured entry does not match, it must remain
    /// recoverable and the resulting target state must be reported as unknown.
    fn remove_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    /// Create one directory entry without following symbolic-link ancestors.
    fn create_directory_checked<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        create_directory_options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata>;

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>>;

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        remove_options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;

    fn copy<'a>(
        &'a self,
        source_path: &'a PathUri,
        destination_path: &'a PathUri,
        copy_options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()>;
}
