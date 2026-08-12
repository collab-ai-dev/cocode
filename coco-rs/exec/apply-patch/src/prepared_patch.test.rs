use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use coco_exec_server::CopyOptions;
use coco_exec_server::CreateDirectoryOptions;
use coco_exec_server::ExecutorFileSystem;
use coco_exec_server::ExecutorFileSystemFuture;
use coco_exec_server::ExpectedFileState;
use coco_exec_server::FileMetadata;
use coco_exec_server::FileSnapshot;
use coco_exec_server::FileSystemReadStream;
use coco_exec_server::FileSystemSandboxContext;
use coco_exec_server::LOCAL_FS;
use coco_exec_server::ReadDirectoryEntry;
use coco_exec_server::RemoveOptions;
use coco_utils_path_uri::PathUri;
use tempfile::TempDir;

use super::*;
use crate::parse_patch;

struct FailNthWrite {
    write_count: AtomicUsize,
    fail_at: usize,
    create_before_first_write: Option<PathBuf>,
    modify_after_first_write: Option<(PathBuf, Vec<u8>)>,
    remove_then_fail: bool,
}

impl ExecutorFileSystem for FailNthWrite {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        LOCAL_FS.canonicalize(path, sandbox)
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        LOCAL_FS.read_file(path, sandbox)
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        LOCAL_FS.read_file_stream(path, sandbox)
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        let write_number = self.write_count.fetch_add(1, Ordering::SeqCst) + 1;
        if write_number == 1
            && let Some(directory) = &self.create_before_first_write
            && let Err(error) = fs::create_dir_all(directory)
        {
            return Box::pin(async move { Err(error) });
        }
        let should_fail = write_number == self.fail_at;
        let modify_after_write = (write_number == 1)
            .then(|| self.modify_after_first_write.clone())
            .flatten();
        Box::pin(async move {
            if should_fail {
                return Err(io::Error::other("injected write failure"));
            }
            LOCAL_FS.write_file(path, contents, sandbox).await?;
            if let Some((path, contents)) = modify_after_write {
                fs::write(path, contents)?;
            }
            Ok(())
        })
    }

    fn snapshot_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSnapshot> {
        LOCAL_FS.snapshot_file(path, sandbox)
    }

    fn write_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        let write_number = self.write_count.fetch_add(1, Ordering::SeqCst) + 1;
        if write_number == 1
            && let Some(directory) = &self.create_before_first_write
            && let Err(error) = fs::create_dir_all(directory)
        {
            return Box::pin(async move { Err(error) });
        }
        let should_fail = write_number == self.fail_at;
        let modify_after_write = (write_number == 1)
            .then(|| self.modify_after_first_write.clone())
            .flatten();
        Box::pin(async move {
            if should_fail {
                return Err(io::Error::other("injected write failure"));
            }
            LOCAL_FS
                .write_file_checked(path, contents, expected, sandbox)
                .await?;
            if let Some((path, contents)) = modify_after_write {
                fs::write(path, contents)?;
            }
            Ok(())
        })
    }

    fn remove_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move {
            LOCAL_FS
                .remove_file_checked(path, expected, sandbox)
                .await?;
            if self.remove_then_fail {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected lost remove response",
                ));
            }
            Ok(())
        })
    }

    fn create_directory_checked<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.create_directory_checked(path, sandbox)
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.create_directory(path, options, sandbox)
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        LOCAL_FS.get_metadata(path, sandbox)
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        LOCAL_FS.read_directory(path, sandbox)
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.remove(path, options, sandbox)
    }

    fn copy<'a>(
        &'a self,
        source: &'a PathUri,
        destination: &'a PathUri,
        options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.copy(source, destination, options, sandbox)
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum RetargetTrigger {
    AfterFirstSuccessfulCanonicalize,
    BeforeFirstWrite,
}

#[cfg(unix)]
struct RetargetParent {
    retargeted: AtomicBool,
    link: PathBuf,
    new_target: PathBuf,
    trigger: RetargetTrigger,
}

#[cfg(unix)]
impl RetargetParent {
    fn retarget_once(&self) -> io::Result<()> {
        if self.retargeted.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        fs::remove_file(&self.link)
            .and_then(|()| std::os::unix::fs::symlink(&self.new_target, &self.link))
    }
}

#[cfg(unix)]
impl ExecutorFileSystem for RetargetParent {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        Box::pin(async move {
            let resolved = LOCAL_FS.canonicalize(path, sandbox).await?;
            if self.trigger == RetargetTrigger::AfterFirstSuccessfulCanonicalize {
                self.retarget_once()?;
            }
            Ok(resolved)
        })
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        LOCAL_FS.read_file(path, sandbox)
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        LOCAL_FS.read_file_stream(path, sandbox)
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        if self.trigger == RetargetTrigger::BeforeFirstWrite
            && let Err(error) = self.retarget_once()
        {
            return Box::pin(async move { Err(error) });
        }
        LOCAL_FS.write_file(path, contents, sandbox)
    }

    fn snapshot_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSnapshot> {
        LOCAL_FS.snapshot_file(path, sandbox)
    }

    fn write_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        if self.trigger == RetargetTrigger::BeforeFirstWrite
            && let Err(error) = self.retarget_once()
        {
            return Box::pin(async move { Err(error) });
        }
        LOCAL_FS.write_file_checked(path, contents, expected, sandbox)
    }

    fn remove_file_checked<'a>(
        &'a self,
        path: &'a PathUri,
        expected: ExpectedFileState,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.remove_file_checked(path, expected, sandbox)
    }

    fn create_directory_checked<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.create_directory_checked(path, sandbox)
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.create_directory(path, options, sandbox)
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        LOCAL_FS.get_metadata(path, sandbox)
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        LOCAL_FS.read_directory(path, sandbox)
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.remove(path, options, sandbox)
    }

    fn copy<'a>(
        &'a self,
        source: &'a PathUri,
        destination: &'a PathUri,
        options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        LOCAL_FS.copy(source, destination, options, sandbox)
    }
}

fn cwd(dir: &TempDir) -> PathUri {
    PathUri::from_path(dir.path()).expect("temporary directory is absolute")
}

async fn prepare(dir: &TempDir, patch: &str) -> Result<PreparedPatch, PreparedPatchError> {
    let parsed = parse_patch(patch).expect("valid patch");
    prepare_hunks(
        &parsed.hunks,
        &cwd(dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        LOCAL_FS.clone(),
        None,
    )
    .await
}

#[tokio::test]
async fn preparation_failure_does_not_apply_earlier_hunks() {
    let dir = TempDir::new().expect("create temp directory");
    let error = prepare(
        &dir,
        "*** Begin Patch\n*** Add File: created.txt\n+created\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch",
    )
    .await
    .expect_err("missing update source must fail");

    assert!(error.to_string().contains("does not exist"));
    assert!(!dir.path().join("created.txt").exists());
}

#[tokio::test]
async fn add_over_non_utf8_target_is_rejected_before_mutation() {
    let dir = TempDir::new().expect("create temp directory");
    let target = dir.path().join("binary.dat");
    let original = vec![0xff, 0xfe, 0xfd];
    fs::write(&target, &original).expect("write binary target");

    let error = prepare(
        &dir,
        "*** Begin Patch\n*** Add File: binary.dat\n+replacement\n*** End Patch",
    )
    .await
    .expect_err("binary overwrite must fail during preparation");

    assert!(error.to_string().contains("not UTF-8 text"));
    assert_eq!(fs::read(target).expect("read unchanged target"), original);
}

#[tokio::test]
async fn prepared_and_committed_debug_output_omit_file_contents() {
    let dir = TempDir::new().expect("create temp directory");
    let secret = "API_KEY=secret-material-that-must-not-be-logged";
    let patch = format!("*** Begin Patch\n*** Add File: secret.txt\n+{secret}\n*** End Patch");

    let prepared = prepare(&dir, &patch).await.expect("prepare patch");
    assert!(!format!("{prepared:?}").contains(secret));

    let committed = commit_prepared_patch(&prepared)
        .await
        .expect("commit patch");
    assert!(!format!("{committed:?}").contains(secret));
}

#[tokio::test]
async fn stale_source_is_rejected_before_commit() {
    let dir = TempDir::new().expect("create temp directory");
    let source = dir.path().join("source.txt");
    fs::write(&source, "old\n").expect("write source");
    let prepared = prepare(
        &dir,
        "*** Begin Patch\n*** Update File: source.txt\n@@\n-old\n+new\n*** End Patch",
    )
    .await
    .expect("prepare patch");
    fs::write(&source, "changed\n").expect("change source");

    let error = commit_prepared_patch(&prepared)
        .await
        .expect_err("stale source must fail");
    assert!(error.to_string().contains("changed after validation"));
    assert_eq!(
        fs::read_to_string(source).expect("read source"),
        "changed\n"
    );
}

#[tokio::test]
async fn commit_failure_reports_committed_prefix_without_unsafe_rollback() {
    let dir = TempDir::new().expect("create temp directory");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Add File: nested/created.txt\n+created\n*** Add File: second.txt\n+fails\n*** End Patch",
    )
    .expect("valid patch");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(FailNthWrite {
        write_count: AtomicUsize::new(0),
        fail_at: 2,
        create_before_first_write: None,
        modify_after_first_write: None,
        remove_then_fail: false,
    });
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    let error = commit_prepared_patch(&prepared)
        .await
        .expect_err("second write must fail");
    assert!(
        error.to_string().contains("write patch target")
            && error.to_string().contains("second.txt"),
        "unexpected error: {error}"
    );
    assert_eq!(error.delta().changes().len(), 1);
    assert!(!error.delta().is_exact());
    assert_eq!(
        fs::read_to_string(dir.path().join("nested/created.txt")).expect("read first write"),
        "created\n"
    );
    assert!(dir.path().join("nested").is_dir());
    assert!(!dir.path().join("second.txt").exists());
}

#[tokio::test]
async fn lost_delete_response_marks_delta_inexact() {
    let dir = TempDir::new().expect("create temp directory");
    let target = dir.path().join("delete.txt");
    fs::write(&target, "before\n").expect("write target");
    let parsed = parse_patch("*** Begin Patch\n*** Delete File: delete.txt\n*** End Patch")
        .expect("valid patch");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(FailNthWrite {
        write_count: AtomicUsize::new(0),
        fail_at: usize::MAX,
        create_before_first_write: None,
        modify_after_first_write: None,
        remove_then_fail: true,
    });
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    let error = commit_prepared_patch(&prepared)
        .await
        .expect_err("injected transport loss must surface");

    assert!(!target.exists(), "executor completed the unlink");
    assert!(error.delta().changes().is_empty());
    assert!(!error.delta().is_exact());
}

#[tokio::test]
async fn lost_move_remove_response_marks_provisional_delta_inexact() {
    let dir = TempDir::new().expect("create temp directory");
    let source = dir.path().join("source.txt");
    let destination = dir.path().join("destination.txt");
    fs::write(&source, "before\n").expect("write source");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Update File: source.txt\n*** Move to: destination.txt\n@@\n-before\n+after\n*** End Patch",
    )
    .expect("valid patch");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(FailNthWrite {
        write_count: AtomicUsize::new(0),
        fail_at: usize::MAX,
        create_before_first_write: None,
        modify_after_first_write: None,
        remove_then_fail: true,
    });
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    let error = commit_prepared_patch(&prepared)
        .await
        .expect_err("injected transport loss must surface");

    assert!(!source.exists(), "executor completed the source unlink");
    assert_eq!(
        fs::read_to_string(destination).expect("read destination"),
        "after\n"
    );
    assert_eq!(error.delta().changes().len(), 1);
    assert!(!error.delta().is_exact());
}

#[tokio::test]
async fn checked_directory_creation_rejects_concurrent_parent_creation() {
    let dir = TempDir::new().expect("create temp directory");
    let external_directory = dir.path().join("nested");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Add File: nested/created.txt\n+created\n*** Add File: second.txt\n+fails\n*** End Patch",
    )
    .expect("valid patch");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(FailNthWrite {
        write_count: AtomicUsize::new(0),
        fail_at: 2,
        create_before_first_write: Some(external_directory.clone()),
        modify_after_first_write: None,
        remove_then_fail: false,
    });
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    commit_prepared_patch(&prepared)
        .await
        .expect_err("second write must fail");

    assert!(external_directory.is_dir());
    assert_eq!(
        fs::read_to_string(external_directory.join("created.txt")).expect("read first write"),
        "created\n"
    );
    assert!(!dir.path().join("second.txt").exists());
}

#[tokio::test]
async fn commit_rechecks_each_target_after_earlier_writes() {
    let dir = TempDir::new().expect("create temp directory");
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    fs::write(&second, "before\n").expect("write second source");
    let parsed = parse_patch(
        "*** Begin Patch\n*** Add File: first.txt\n+created\n*** Update File: second.txt\n@@\n-before\n+patched\n*** End Patch",
    )
    .expect("valid patch");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(FailNthWrite {
        write_count: AtomicUsize::new(0),
        fail_at: usize::MAX,
        create_before_first_write: None,
        modify_after_first_write: Some((second.clone(), b"external\n".to_vec())),
        remove_then_fail: false,
    });
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    let error = commit_prepared_patch(&prepared)
        .await
        .expect_err("external update must make the second target stale");

    let (error, delta) = error.into_parts();
    assert!(matches!(error, PreparedPatchError::StaleTarget(_)));
    assert_eq!(delta.changes().len(), 1);
    assert_eq!(fs::read_to_string(first).expect("read first"), "created\n");
    assert_eq!(
        fs::read_to_string(second).expect("read second"),
        "external\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symbolic_link_targets() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    fs::write(dir.path().join("real.txt"), "old\n").expect("write source");
    symlink("real.txt", dir.path().join("alias.txt")).expect("create symlink");

    let error = prepare(
        &dir,
        "*** Begin Patch\n*** Update File: alias.txt\n@@\n-old\n+new\n*** End Patch",
    )
    .await
    .expect_err("symlink must fail");
    assert!(error.to_string().contains("symbolic link"));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_hard_link_targets() {
    let dir = TempDir::new().expect("create temp directory");
    let first = dir.path().join("first.txt");
    fs::write(&first, "old\n").expect("write source");
    fs::hard_link(&first, dir.path().join("alias.txt")).expect("create hard link");

    let error = prepare(
        &dir,
        "*** Begin Patch\n*** Update File: first.txt\n@@\n-old\n+new\n*** End Patch",
    )
    .await
    .expect_err("hard-linked target must fail");

    assert!(error.to_string().contains("multiple hard links"));
    assert_eq!(fs::read_to_string(first).expect("read source"), "old\n");
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_dangling_symbolic_link_targets() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    symlink("missing.txt", dir.path().join("alias.txt")).expect("create dangling symlink");

    let error = prepare(
        &dir,
        "*** Begin Patch\n*** Add File: alias.txt\n+new\n*** End Patch",
    )
    .await
    .expect_err("dangling symlink must fail");
    assert!(error.to_string().contains("symbolic link"));
    assert!(fs::symlink_metadata(dir.path().join("alias.txt")).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn parent_retarget_after_preparation_cannot_redirect_commit() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    let first_target = TempDir::new().expect("create first target");
    let second_target = TempDir::new().expect("create second target");
    let link = dir.path().join("linked");
    symlink(first_target.path(), &link).expect("create parent symlink");

    let prepared = prepare(
        &dir,
        "*** Begin Patch\n*** Add File: linked/created.txt\n+created\n*** End Patch",
    )
    .await
    .expect("prepare patch");

    fs::remove_file(&link).expect("remove original parent symlink");
    symlink(second_target.path(), &link).expect("retarget parent symlink");

    commit_prepared_patch(&prepared)
        .await
        .expect("commit must use prepared canonical target");
    assert_eq!(
        fs::read_to_string(first_target.path().join("created.txt")).expect("read target"),
        "created\n"
    );
    assert!(!second_target.path().join("created.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn commit_uses_canonical_path_if_parent_is_retargeted_after_validation() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    let validated_target = TempDir::new().expect("create validated target");
    let attacker_target = TempDir::new().expect("create attacker target");
    let link = dir.path().join("linked");
    symlink(validated_target.path(), &link).expect("create parent symlink");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(RetargetParent {
        retargeted: AtomicBool::new(false),
        link,
        new_target: attacker_target.path().to_path_buf(),
        trigger: RetargetTrigger::BeforeFirstWrite,
    });
    let parsed =
        parse_patch("*** Begin Patch\n*** Add File: linked/created.txt\n+created\n*** End Patch")
            .expect("valid patch");
    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    commit_prepared_patch(&prepared)
        .await
        .expect("commit through validated canonical path");

    assert_eq!(
        fs::read_to_string(validated_target.path().join("created.txt"))
            .expect("read validated target"),
        "created\n"
    );
    assert!(!attacker_target.path().join("created.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn preparation_reuses_the_path_resolution_checked_by_policy() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create temp directory");
    let validated_target = TempDir::new().expect("create validated target");
    let attacker_target = TempDir::new().expect("create attacker target");
    let link = dir.path().join("linked");
    symlink(validated_target.path(), &link).expect("create parent symlink");
    let fs: Arc<dyn ExecutorFileSystem> = Arc::new(RetargetParent {
        retargeted: AtomicBool::new(false),
        link,
        new_target: attacker_target.path().to_path_buf(),
        trigger: RetargetTrigger::AfterFirstSuccessfulCanonicalize,
    });
    let parsed =
        parse_patch("*** Begin Patch\n*** Add File: linked/created.txt\n+created\n*** End Patch")
            .expect("valid patch");

    let prepared = prepare_hunks(
        &parsed.hunks,
        &cwd(&dir),
        ApplyPatchFileUpdateMode::PreserveLineEndings,
        fs,
        None,
    )
    .await
    .expect("prepare patch");

    let permission_path = prepared
        .path_effects()
        .paths()
        .first()
        .expect("permission path");
    let proposed_path = prepared
        .proposed_writes()
        .next()
        .map(|(path, _)| path)
        .expect("proposed write");
    let expected =
        PathUri::from_path(validated_target.path().join("created.txt")).expect("absolute path");
    assert_eq!(permission_path, &expected);
    assert_eq!(proposed_path, &expected);

    commit_prepared_patch(&prepared)
        .await
        .expect("commit must use the authorized canonical plan");
    assert_eq!(
        fs::read_to_string(validated_target.path().join("created.txt"))
            .expect("read validated target"),
        "created\n"
    );
    assert!(!attacker_target.path().join("created.txt").exists());
}
