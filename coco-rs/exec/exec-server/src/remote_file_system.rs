use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use coco_utils_path_uri::PathUri;
use tokio::io;
use tracing::trace;

use crate::CopyOptions;
use crate::CreateDirectoryOptions;
use crate::ExecServerError;
use crate::ExecutorFileSystem;
use crate::ExecutorFileSystemFuture;
use crate::ExpectedFileState;
use crate::FileMetadata;
use crate::FileSnapshot;
use crate::FileSystemReadStream;
use crate::FileSystemResult;
use crate::FileSystemSandboxContext;
use crate::ReadDirectoryEntry;
use crate::RemoveOptions;
use crate::client::LazyRemoteExecServerClient;
use crate::file_mutation_conflict;
use crate::protocol::FsCanonicalizeParams;
use crate::protocol::FsCopyParams;
use crate::protocol::FsCreateDirectoryCheckedParams;
use crate::protocol::FsCreateDirectoryParams;
use crate::protocol::FsGetMetadataParams;
use crate::protocol::FsReadDirectoryParams;
use crate::protocol::FsReadFileParams;
use crate::protocol::FsRemoveFileCheckedParams;
use crate::protocol::FsRemoveParams;
use crate::protocol::FsSnapshotFileParams;
use crate::protocol::FsWriteFileCheckedParams;
use crate::protocol::FsWriteFileParams;

const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const NOT_FOUND_ERROR_CODE: i64 = -32004;

#[path = "remote_file_stream.rs"]
mod file_stream;

pub(crate) struct RemoteFileSystem {
    client: LazyRemoteExecServerClient,
}

impl RemoteFileSystem {
    pub(crate) fn new(client: LazyRemoteExecServerClient) -> Self {
        trace!("remote fs new");
        Self { client }
    }

    async fn canonicalize(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<PathUri> {
        trace!("remote fs canonicalize");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_canonicalize(FsCanonicalizeParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(response.path)
    }

    async fn read_file(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        trace!("remote fs read_file");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_read_file(FsReadFileParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        STANDARD.decode(response.data_base64).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("remote fs/readFile returned invalid base64 dataBase64: {err}"),
            )
        })
    }

    async fn read_file_stream(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileSystemReadStream> {
        if sandbox.is_some_and(FileSystemSandboxContext::should_run_in_sandbox) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "streaming file reads do not support platform sandboxing",
            ));
        }
        trace!("remote fs read_file_stream");
        let client = self.client.get().await.map_err(map_remote_error)?;
        file_stream::open(client, path.clone(), remote_sandbox_context(sandbox)).await
    }

    async fn write_file(
        &self,
        path: &PathUri,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs write_file");
        let client = self.client.get().await.map_err(map_remote_error)?;
        client
            .fs_write_file(FsWriteFileParams {
                path: path.clone(),
                data_base64: STANDARD.encode(contents),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(())
    }

    async fn snapshot_file(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileSnapshot> {
        trace!("remote fs snapshot_file");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_snapshot_file(FsSnapshotFileParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        let contents = response
            .data_base64
            .map(|data| {
                STANDARD.decode(data).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "remote fs/snapshotFile returned invalid base64 dataBase64: {error}"
                        ),
                    )
                })
            })
            .transpose()?;
        match (&response.expected, &contents) {
            (ExpectedFileState::Missing, None) | (ExpectedFileState::File { .. }, Some(_)) => {
                Ok(FileSnapshot {
                    expected: response.expected,
                    contents,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remote fs/snapshotFile returned inconsistent state and contents",
            )),
        }
    }

    async fn write_file_checked(
        &self,
        path: &PathUri,
        contents: Vec<u8>,
        expected: ExpectedFileState,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs write_file_checked");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_write_file_checked(FsWriteFileCheckedParams {
                path: path.clone(),
                data_base64: STANDARD.encode(contents),
                expected,
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        if response.applied {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    async fn remove_file_checked(
        &self,
        path: &PathUri,
        expected: ExpectedFileState,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs remove_file_checked");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_remove_file_checked(FsRemoveFileCheckedParams {
                path: path.clone(),
                expected,
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        if response.applied {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    async fn create_directory_checked(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs create_directory_checked");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_create_directory_checked(FsCreateDirectoryCheckedParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        if response.applied {
            Ok(())
        } else {
            Err(file_mutation_conflict())
        }
    }

    async fn create_directory(
        &self,
        path: &PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs create_directory");
        let client = self.client.get().await.map_err(map_remote_error)?;
        client
            .fs_create_directory(FsCreateDirectoryParams {
                path: path.clone(),
                recursive: Some(options.recursive),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        trace!("remote fs get_metadata");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_get_metadata(FsGetMetadataParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(FileMetadata {
            is_directory: response.is_directory,
            is_file: response.is_file,
            is_symlink: response.is_symlink,
            size: response.size,
            created_at_ms: response.created_at_ms,
            modified_at_ms: response.modified_at_ms,
        })
    }

    async fn read_directory(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        trace!("remote fs read_directory");
        let client = self.client.get().await.map_err(map_remote_error)?;
        let response = client
            .fs_read_directory(FsReadDirectoryParams {
                path: path.clone(),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(response
            .entries
            .into_iter()
            .map(|entry| ReadDirectoryEntry {
                file_name: entry.file_name,
                is_directory: entry.is_directory,
                is_file: entry.is_file,
            })
            .collect())
    }

    async fn remove(
        &self,
        path: &PathUri,
        options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs remove");
        let client = self.client.get().await.map_err(map_remote_error)?;
        client
            .fs_remove(FsRemoveParams {
                path: path.clone(),
                recursive: Some(options.recursive),
                force: Some(options.force),
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(())
    }

    async fn copy(
        &self,
        source_path: &PathUri,
        destination_path: &PathUri,
        options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        trace!("remote fs copy");
        let client = self.client.get().await.map_err(map_remote_error)?;
        client
            .fs_copy(FsCopyParams {
                source_path: source_path.clone(),
                destination_path: destination_path.clone(),
                recursive: options.recursive,
                sandbox: remote_sandbox_context(sandbox),
            })
            .await
            .map_err(map_remote_error)?;
        Ok(())
    }
}

impl ExecutorFileSystem for RemoteFileSystem {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        Box::pin(RemoteFileSystem::canonicalize(self, path, sandbox))
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        Box::pin(RemoteFileSystem::read_file(self, path, sandbox))
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        Box::pin(RemoteFileSystem::read_file_stream(self, path, sandbox))
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::write_file(self, path, contents, sandbox))
    }

    fn snapshot_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSnapshot> {
        Box::pin(RemoteFileSystem::snapshot_file(self, path, sandbox))
    }

    fn write_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::write_file_checked(
            self, path, contents, expected, sandbox,
        ))
    }

    fn remove_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::remove_file_checked(
            self, path, expected, sandbox,
        ))
    }

    fn create_directory_checked<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::create_directory_checked(
            self, path, sandbox,
        ))
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::create_directory(
            self, path, options, sandbox,
        ))
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        Box::pin(RemoteFileSystem::get_metadata(self, path, sandbox))
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        Box::pin(RemoteFileSystem::read_directory(self, path, sandbox))
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::remove(self, path, options, sandbox))
    }

    fn copy<'a>(
        &'a self,
        source_path: &'a PathUri,
        destination_path: &'a PathUri,
        options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(RemoteFileSystem::copy(
            self,
            source_path,
            destination_path,
            options,
            sandbox,
        ))
    }
}

fn remote_sandbox_context(
    sandbox: Option<&FileSystemSandboxContext>,
) -> Option<FileSystemSandboxContext> {
    sandbox.cloned()
}

pub(crate) fn map_remote_error(error: ExecServerError) -> io::Error {
    match error {
        ExecServerError::Server { code, message } if code == NOT_FOUND_ERROR_CODE => {
            io::Error::new(io::ErrorKind::NotFound, message)
        }
        ExecServerError::Server { code, message } if code == INVALID_REQUEST_ERROR_CODE => {
            io::Error::new(io::ErrorKind::InvalidInput, message)
        }
        ExecServerError::Server { message, .. } => io::Error::other(message),
        ExecServerError::Closed | ExecServerError::Disconnected(_) => {
            io::Error::new(io::ErrorKind::BrokenPipe, "exec-server transport closed")
        }
        _ => io::Error::other(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::io::duplex;

    use super::*;

    async fn connected_remote_file_system() -> (RemoteFileSystem, tokio::task::JoinHandle<()>) {
        let (client_writer, server_reader) = duplex(1 << 20);
        let (server_writer, client_reader) = duplex(1 << 20);
        let runtime_paths = crate::ExecServerRuntimePaths::new(
            std::env::current_exe().expect("current executable"),
            None,
        )
        .expect("runtime paths");
        let processor = crate::server::ConnectionProcessor::new(runtime_paths);
        let server_connection = crate::connection::JsonRpcConnection::from_stdio(
            server_reader,
            server_writer,
            "checked-fs-server".to_string(),
        );
        let server = tokio::spawn(async move {
            processor.run_connection(server_connection).await;
        });
        let client_connection = crate::connection::JsonRpcConnection::from_stdio(
            client_reader,
            client_writer,
            "checked-fs-client".to_string(),
        );
        let client = crate::ExecServerClient::connect(
            client_connection,
            crate::ExecServerClientConnectOptions::default(),
        )
        .await
        .expect("connect exec-server client");
        let lazy = LazyRemoteExecServerClient::from_connected_for_test(client);
        (RemoteFileSystem::new(lazy), server)
    }

    #[test]
    fn transport_errors_map_to_broken_pipe() {
        let errors = [
            ExecServerError::Closed,
            ExecServerError::Disconnected("exec-server transport disconnected".to_string()),
        ];

        let mapped_errors = errors
            .into_iter()
            .map(|error| {
                let error = map_remote_error(error);
                (error.kind(), error.to_string())
            })
            .collect::<Vec<_>>();

        assert_eq!(
            mapped_errors,
            vec![
                (
                    io::ErrorKind::BrokenPipe,
                    "exec-server transport closed".to_string()
                ),
                (
                    io::ErrorKind::BrokenPipe,
                    "exec-server transport closed".to_string()
                ),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn checked_filesystem_round_trips_over_json_rpc() -> io::Result<()> {
        let (fs, server) = connected_remote_file_system().await;
        let temp_dir = tempfile::TempDir::new()?;
        let directory = PathUri::from_path(temp_dir.path().join("nested"))?;
        let target = PathUri::from_path(temp_dir.path().join("nested/target.bin"))?;

        fs.create_directory_checked(&directory, None).await?;
        let missing = fs.snapshot_file(&target, None).await?;
        assert!(matches!(missing.expected, ExpectedFileState::Missing));
        fs.write_file_checked(&target, vec![0, 1, 2, 0xff], missing.expected.clone(), None)
            .await?;

        let first = fs.snapshot_file(&target, None).await?;
        assert_eq!(first.contents.as_deref(), Some([0, 1, 2, 0xff].as_slice()));
        fs.write_file_checked(&target, b"updated".to_vec(), first.expected.clone(), None)
            .await?;
        let stale_write = fs
            .write_file_checked(&target, b"stale".to_vec(), first.expected.clone(), None)
            .await
            .expect_err("stale remote write must conflict");
        assert!(crate::is_file_mutation_conflict(&stale_write));
        let stale_remove = fs
            .remove_file_checked(&target, first.expected, None)
            .await
            .expect_err("stale remote remove must conflict");
        assert!(crate::is_file_mutation_conflict(&stale_remove));

        let current = fs.snapshot_file(&target, None).await?;
        assert_eq!(current.contents.as_deref(), Some(b"updated".as_slice()));
        fs.remove_file_checked(&target, current.expected, None)
            .await?;
        assert!(matches!(
            fs.snapshot_file(&target, None).await?.expected,
            ExpectedFileState::Missing
        ));

        drop(fs);
        server.abort();
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_snapshot_preserves_hard_link_count() -> io::Result<()> {
        let (fs, server) = connected_remote_file_system().await;
        let temp_dir = tempfile::TempDir::new()?;
        let first = temp_dir.path().join("first.txt");
        let second = temp_dir.path().join("second.txt");
        std::fs::write(&first, "shared")?;
        std::fs::hard_link(&first, second)?;

        let snapshot = fs.snapshot_file(&PathUri::from_path(first)?, None).await?;
        let ExpectedFileState::File { version } = snapshot.expected else {
            panic!("existing remote snapshot");
        };
        assert_eq!(version.link_count, 2);

        drop(fs);
        server.abort();
        Ok(())
    }
}
