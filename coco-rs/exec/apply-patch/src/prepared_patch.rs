use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use coco_exec_server::CheckedFileSystem;
use coco_exec_server::ExpectedFileState;
use coco_exec_server::FileSystemSandboxContext;
use coco_exec_server::is_file_mutation_conflict;
use coco_utils_path_uri::PathUri;
use thiserror::Error;

use crate::AffectedPaths;
use crate::AppliedPatchChange;
use crate::AppliedPatchDelta;
use crate::AppliedPatchFileChange;
use crate::ApplyPatchError;
use crate::ApplyPatchFileUpdateMode;
use crate::ApplyPatchPathEffects;
use crate::Hunk;
use crate::PreparedPatchPathOutcome;
use crate::PreparedPatchPathState;
use crate::file_update::derive_new_contents;
use crate::path_effects::ResolvedOperationPath;
use crate::path_effects::bounded_path;
use crate::path_effects::resolve_and_validate_hunk_paths;

#[derive(Debug, Error)]
pub enum PreparedPatchError {
    #[error(transparent)]
    ApplyPatch(#[from] ApplyPatchError),
    #[error("{0}")]
    InvalidTarget(String),
    #[error("patch target changed after validation: {0}")]
    StaleTarget(String),
    #[error("failed to {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
}

struct PathSnapshot {
    logical_path: PathUri,
    resolved_path: PathUri,
    expected: ExpectedFileState,
    original: Option<Vec<u8>>,
}

enum PreparedChange {
    Add {
        path: PathUri,
        content: String,
        overwritten_content: Option<String>,
        summary_path: PathBuf,
    },
    Delete {
        path: PathUri,
        original_content: String,
        summary_path: PathBuf,
    },
    Update {
        source: PathUri,
        destination: Option<PathUri>,
        content: String,
        old_content: String,
        overwritten_move_content: Option<String>,
        summary_path: PathBuf,
    },
}

/// Fully derived, executor-bound commit plan produced after authorization.
/// Its paths come from [`PreparedPatchPaths`], so no target is resolved a
/// second time after the user approves the tool call.
pub struct PreparedPatch {
    fs: Arc<dyn CheckedFileSystem>,
    sandbox: Option<FileSystemSandboxContext>,
    changes: Vec<PreparedChange>,
    snapshots: Vec<PathSnapshot>,
    missing_parent_directories: Vec<PathUri>,
    path_effects: ApplyPatchPathEffects,
}

/// Canonical, executor-bound path plan produced before permission resolution.
///
/// This phase resolves only target identities. It deliberately does not read
/// target contents or test patch context, so an unapproved patch cannot use
/// preparation errors as a file-content oracle. After authorization,
/// [`prepare_hunks_from_paths`] snapshots and derives the immutable commit
/// plan without resolving any path again.
pub struct PreparedPatchPaths {
    fs: Arc<dyn CheckedFileSystem>,
    sandbox: Option<FileSystemSandboxContext>,
    hunks: Vec<Hunk>,
    cwd: PathUri,
    update_file_mode: ApplyPatchFileUpdateMode,
    validated: crate::path_effects::ValidatedHunkPaths,
}

/// Successfully committed writes and deletions.
pub struct CommittedPatch {
    affected_paths: AffectedPaths,
    delta: AppliedPatchDelta,
    written_files: Vec<(PathUri, String)>,
    deleted_files: Vec<PathUri>,
}

/// A commit failure plus the exact textual prefix known to have reached disk.
#[derive(Debug, Error)]
#[error("{error}")]
pub struct PreparedPatchCommitFailure {
    #[source]
    error: PreparedPatchError,
    delta: AppliedPatchDelta,
    path_outcomes: Vec<PreparedPatchPathOutcome>,
}

impl PreparedPatchCommitFailure {
    fn new(
        error: PreparedPatchError,
        delta: AppliedPatchDelta,
        path_outcomes: Vec<PreparedPatchPathOutcome>,
    ) -> Self {
        Self {
            error,
            delta,
            path_outcomes,
        }
    }

    pub fn delta(&self) -> &AppliedPatchDelta {
        &self.delta
    }

    pub fn path_outcomes(&self) -> &[PreparedPatchPathOutcome] {
        &self.path_outcomes
    }

    pub fn into_parts(
        self,
    ) -> (
        PreparedPatchError,
        AppliedPatchDelta,
        Vec<PreparedPatchPathOutcome>,
    ) {
        (self.error, self.delta, self.path_outcomes)
    }
}

impl fmt::Debug for CommittedPatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommittedPatch")
            .field("affected_paths", &self.affected_paths)
            .field("delta_change_count", &self.delta.changes().len())
            .field("written_file_count", &self.written_files.len())
            .field("deleted_file_count", &self.deleted_files.len())
            .finish_non_exhaustive()
    }
}

impl CommittedPatch {
    pub fn affected_paths(&self) -> &AffectedPaths {
        &self.affected_paths
    }

    pub fn delta(&self) -> &AppliedPatchDelta {
        &self.delta
    }

    pub fn written_files(&self) -> impl Iterator<Item = (&PathUri, &str)> {
        self.written_files
            .iter()
            .map(|(path, content)| (path, content.as_str()))
    }

    pub fn deleted_files(&self) -> &[PathUri] {
        &self.deleted_files
    }
}

impl fmt::Debug for PreparedPatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPatch")
            .field("change_count", &self.changes.len())
            .field("snapshot_count", &self.snapshots.len())
            .field(
                "missing_parent_directory_count",
                &self.missing_parent_directories.len(),
            )
            .field("path_effects", &self.path_effects)
            .field("has_sandbox_context", &self.sandbox.is_some())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PreparedPatchPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPatchPaths")
            .field("hunk_count", &self.hunks.len())
            .field("path_effects", &self.validated.path_effects)
            .field("has_sandbox_context", &self.sandbox.is_some())
            .finish_non_exhaustive()
    }
}

impl PreparedPatchPaths {
    pub fn path_effects(&self) -> &ApplyPatchPathEffects {
        &self.validated.path_effects
    }
}

impl PreparedPatch {
    pub fn path_effects(&self) -> &ApplyPatchPathEffects {
        &self.path_effects
    }

    pub fn proposed_writes(&self) -> impl Iterator<Item = (&PathUri, &str)> {
        self.changes.iter().filter_map(|change| match change {
            PreparedChange::Add { path, content, .. } => Some((path, content.as_str())),
            PreparedChange::Delete { .. } => None,
            PreparedChange::Update {
                source,
                destination,
                content,
                ..
            } => Some((destination.as_ref().unwrap_or(source), content.as_str())),
        })
    }
}

/// Validates and derives every hunk without mutating the filesystem.
pub async fn prepare_hunks(
    hunks: &[Hunk],
    cwd: &PathUri,
    update_file_mode: ApplyPatchFileUpdateMode,
    fs: Arc<dyn CheckedFileSystem>,
    sandbox: Option<FileSystemSandboxContext>,
) -> Result<PreparedPatch, PreparedPatchError> {
    let paths = prepare_hunk_paths(hunks, cwd, update_file_mode, fs, sandbox).await?;
    prepare_hunks_from_paths(&paths).await
}

/// Resolve and bind patch target paths without reading target contents.
pub async fn prepare_hunk_paths(
    hunks: &[Hunk],
    cwd: &PathUri,
    update_file_mode: ApplyPatchFileUpdateMode,
    fs: Arc<dyn CheckedFileSystem>,
    sandbox: Option<FileSystemSandboxContext>,
) -> Result<PreparedPatchPaths, PreparedPatchError> {
    if hunks.is_empty() {
        return Err(PreparedPatchError::InvalidTarget(
            "No files were modified.".to_string(),
        ));
    }

    let validated =
        resolve_and_validate_hunk_paths(hunks, cwd, fs.as_ref(), sandbox.as_ref()).await?;
    Ok(PreparedPatchPaths {
        fs,
        sandbox,
        hunks: hunks.to_vec(),
        cwd: cwd.clone(),
        update_file_mode,
        validated,
    })
}

/// Complete preparation after authorization, reusing the exact canonical path
/// identities that permission evaluation inspected.
pub async fn prepare_hunks_from_paths(
    paths: &PreparedPatchPaths,
) -> Result<PreparedPatch, PreparedPatchError> {
    let fs = Arc::clone(&paths.fs);
    let sandbox = paths.sandbox.clone();
    let sandbox_ref = sandbox.as_ref();
    let hunks = paths.hunks.as_slice();
    let cwd = &paths.cwd;
    let mut snapshots = Vec::with_capacity(paths.validated.operations.len());
    for operation in &paths.validated.operations {
        snapshots.push(snapshot_resolved_path(operation, fs.as_ref(), sandbox_ref).await?);
    }
    let path_effects = paths.validated.path_effects.clone();

    let mut changes = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let source = hunk.resolve_path(cwd).map_err(ApplyPatchError::from)?;
        match hunk {
            Hunk::AddFile { contents, .. } => {
                let path = resolved_path_for(&snapshots, &source)?.clone();
                let overwritten_content = snapshot_text(snapshot_for_resolved(&snapshots, &path)?)?;
                changes.push(PreparedChange::Add {
                    path,
                    content: contents.clone(),
                    overwritten_content,
                    summary_path: hunk.path().to_path_buf(),
                });
            }
            Hunk::DeleteFile { .. } => {
                let original_content =
                    String::from_utf8(require_existing_file(&snapshots, &source)?.to_vec())
                        .map_err(|error| non_utf8_target(&source, error))?;
                changes.push(PreparedChange::Delete {
                    path: resolved_path_for(&snapshots, &source)?.clone(),
                    original_content,
                    summary_path: hunk.path().to_path_buf(),
                });
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                let old_content =
                    String::from_utf8(require_existing_file(&snapshots, &source)?.to_vec())
                        .map_err(|error| non_utf8_target(&source, error))?;
                let content = derive_new_contents(
                    old_content.clone(),
                    &source.inferred_native_path_string(),
                    chunks,
                    paths.update_file_mode,
                )?
                .new_contents;
                let destination = move_path
                    .as_ref()
                    .map(|path| cwd.join(&path.to_string_lossy()))
                    .transpose()
                    .map_err(ApplyPatchError::from)?;
                let resolved_source = resolved_path_for(&snapshots, &source)?.clone();
                let resolved_destination = destination
                    .as_ref()
                    .map(|path| resolved_path_for(&snapshots, path).cloned())
                    .transpose()?;
                let overwritten_move_content = resolved_destination
                    .as_ref()
                    .map(|path| snapshot_for_resolved(&snapshots, path).and_then(snapshot_text))
                    .transpose()?
                    .flatten();
                changes.push(PreparedChange::Update {
                    source: resolved_source,
                    destination: resolved_destination,
                    content,
                    old_content,
                    overwritten_move_content,
                    summary_path: hunk.path().to_path_buf(),
                });
            }
        }
    }

    let missing_parent_directories =
        collect_missing_parent_directories(&changes, fs.as_ref(), sandbox_ref).await?;
    Ok(PreparedPatch {
        fs,
        sandbox,
        changes,
        snapshots,
        missing_parent_directories,
        path_effects,
    })
}

/// Commits the exact plan authorized by the runtime. Checked mutations make
/// target validation and mutation one executor-owned operation. On failure,
/// already committed changes are reported instead of being unsafely rolled
/// back over possible external edits.
pub async fn commit_prepared_patch(
    prepared: &PreparedPatch,
) -> Result<CommittedPatch, PreparedPatchCommitFailure> {
    let fs = prepared.fs.as_ref();
    let sandbox = prepared.sandbox.as_ref();
    let mut path_outcomes = prepared
        .snapshots
        .iter()
        .map(|snapshot| PreparedPatchPathOutcome {
            path: snapshot.resolved_path.clone(),
            state: PreparedPatchPathState::Unchanged,
        })
        .collect::<Vec<_>>();
    if let Err(error) = verify_snapshots(&prepared.snapshots, &mut path_outcomes, fs, sandbox).await
    {
        return Err(PreparedPatchCommitFailure::new(
            error,
            AppliedPatchDelta::empty(),
            path_outcomes,
        ));
    }
    if let Err(error) =
        create_missing_directories(&prepared.missing_parent_directories, fs, sandbox).await
    {
        return Err(PreparedPatchCommitFailure::new(
            error,
            AppliedPatchDelta::empty(),
            path_outcomes,
        ));
    }

    let mut affected = AffectedPaths::default();
    let mut delta = AppliedPatchDelta::empty();
    let mut written_files = Vec::new();
    let mut deleted_files = Vec::new();
    for change in &prepared.changes {
        if let Err(error) = commit_change(
            change,
            &prepared.snapshots,
            &mut affected,
            &mut delta,
            &mut written_files,
            &mut deleted_files,
            &mut path_outcomes,
            fs,
            sandbox,
        )
        .await
        {
            return Err(PreparedPatchCommitFailure::new(error, delta, path_outcomes));
        }
    }

    Ok(CommittedPatch {
        affected_paths: affected,
        delta,
        written_files,
        deleted_files,
    })
}

async fn snapshot_resolved_path(
    operation: &ResolvedOperationPath,
    fs: &dyn CheckedFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<PathSnapshot, PreparedPatchError> {
    match fs.get_metadata(&operation.logical_path, sandbox).await {
        Ok(metadata) if metadata.is_symlink => {
            return Err(PreparedPatchError::InvalidTarget(format!(
                "patch target is a symbolic link: {}",
                bounded_path(&operation.logical_path)
            )));
        }
        Ok(metadata) if metadata.is_directory || !metadata.is_file => {
            return Err(PreparedPatchError::InvalidTarget(format!(
                "patch target is not a regular file: {}",
                bounded_path(&operation.logical_path)
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(io_error(
                "inspect patch target",
                &operation.logical_path,
                source,
            ));
        }
    }

    let snapshot = fs
        .snapshot_file(&operation.resolved.path, sandbox)
        .await
        .map_err(|source| io_error("snapshot patch target", &operation.resolved.path, source))?;
    if let ExpectedFileState::File { version } = &snapshot.expected
        && version.link_count > 1
    {
        return Err(PreparedPatchError::InvalidTarget(format!(
            "patch target has multiple hard links: {}",
            bounded_path(&operation.logical_path)
        )));
    }
    Ok(PathSnapshot {
        logical_path: operation.logical_path.clone(),
        resolved_path: operation.resolved.path.clone(),
        expected: snapshot.expected,
        original: snapshot.contents,
    })
}

fn resolved_path_for<'a>(
    snapshots: &'a [PathSnapshot],
    path: &PathUri,
) -> Result<&'a PathUri, PreparedPatchError> {
    snapshots
        .iter()
        .find(|snapshot| &snapshot.logical_path == path)
        .map(|snapshot| &snapshot.resolved_path)
        .ok_or_else(|| PreparedPatchError::StaleTarget(bounded_path(path)))
}

fn snapshot_for_resolved<'a>(
    snapshots: &'a [PathSnapshot],
    path: &PathUri,
) -> Result<&'a PathSnapshot, PreparedPatchError> {
    snapshots
        .iter()
        .find(|snapshot| &snapshot.resolved_path == path)
        .ok_or_else(|| PreparedPatchError::StaleTarget(bounded_path(path)))
}

fn require_existing_file<'a>(
    snapshots: &'a [PathSnapshot],
    path: &PathUri,
) -> Result<&'a [u8], PreparedPatchError> {
    snapshots
        .iter()
        .find(|snapshot| &snapshot.logical_path == path)
        .and_then(|snapshot| snapshot.original.as_deref())
        .ok_or_else(|| {
            PreparedPatchError::InvalidTarget(format!(
                "patch source does not exist: {}",
                bounded_path(path)
            ))
        })
}

async fn collect_missing_parent_directories(
    changes: &[PreparedChange],
    fs: &dyn CheckedFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<Vec<PathUri>, PreparedPatchError> {
    let mut directories = Vec::new();
    let mut seen = HashSet::new();
    for path in changes.iter().filter_map(write_path) {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            match fs.get_metadata(&directory, sandbox).await {
                Ok(metadata) if metadata.is_directory && !metadata.is_symlink => break,
                Ok(_) => {
                    return Err(PreparedPatchError::InvalidTarget(format!(
                        "patch parent is not a regular directory: {}",
                        bounded_path(&directory)
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if seen.insert(directory.clone()) {
                        directories.push(directory.clone());
                    }
                    parent = directory.parent();
                }
                Err(source) => {
                    return Err(io_error("inspect parent directory", &directory, source));
                }
            }
        }
    }
    directories.sort_by_key(|path| path.encoded_path().len());
    Ok(directories)
}

fn write_path(change: &PreparedChange) -> Option<&PathUri> {
    match change {
        PreparedChange::Add { path, .. } => Some(path),
        PreparedChange::Delete { .. } => None,
        PreparedChange::Update {
            source,
            destination,
            ..
        } => Some(destination.as_ref().unwrap_or(source)),
    }
}

async fn verify_snapshots(
    snapshots: &[PathSnapshot],
    path_outcomes: &mut [PreparedPatchPathOutcome],
    fs: &dyn CheckedFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<(), PreparedPatchError> {
    for expected in snapshots {
        let current = match fs.snapshot_file(&expected.resolved_path, sandbox).await {
            Ok(current) => current,
            Err(source) => {
                set_path_state(
                    path_outcomes,
                    &expected.resolved_path,
                    PreparedPatchPathState::Unknown,
                );
                return Err(io_error(
                    "verify patch target",
                    &expected.resolved_path,
                    source,
                ));
            }
        };
        if current.expected != expected.expected {
            set_path_state(
                path_outcomes,
                &expected.resolved_path,
                PreparedPatchPathState::StaleExternal,
            );
            return Err(PreparedPatchError::StaleTarget(bounded_path(
                &expected.logical_path,
            )));
        }
    }
    Ok(())
}

async fn create_missing_directories(
    directories: &[PathUri],
    fs: &dyn CheckedFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<(), PreparedPatchError> {
    for directory in directories {
        fs.create_directory_checked(directory, sandbox)
            .await
            .map_err(|source| checked_error("create parent directory", directory, source))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn commit_change(
    change: &PreparedChange,
    snapshots: &[PathSnapshot],
    affected: &mut AffectedPaths,
    delta: &mut AppliedPatchDelta,
    written_files: &mut Vec<(PathUri, String)>,
    deleted_files: &mut Vec<PathUri>,
    path_outcomes: &mut [PreparedPatchPathOutcome],
    fs: &dyn CheckedFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<(), PreparedPatchError> {
    match change {
        PreparedChange::Add {
            path,
            content,
            overwritten_content,
            summary_path,
        } => {
            let snapshot = snapshot_for_resolved(snapshots, path)?;
            if let Err(source) = fs
                .write_file_checked(
                    path,
                    content.as_bytes().to_vec(),
                    snapshot.expected.clone(),
                    sandbox,
                )
                .await
            {
                set_path_state(path_outcomes, path, checked_failure_state(&source));
                delta.exact = false;
                return Err(checked_error("write patch target", path, source));
            }
            set_path_state(
                path_outcomes,
                path,
                PreparedPatchPathState::Written {
                    content: content.clone(),
                },
            );
            delta.changes.push(AppliedPatchChange {
                path: path.clone(),
                change: AppliedPatchFileChange::Add {
                    content: content.clone(),
                    overwritten_content: overwritten_content.clone(),
                },
            });
            written_files.push((path.clone(), content.clone()));
            affected.added.push(summary_path.clone());
        }
        PreparedChange::Delete {
            path,
            original_content,
            summary_path,
        } => {
            let snapshot = snapshot_for_resolved(snapshots, path)?;
            if let Err(source) = fs
                .remove_file_checked(path, snapshot.expected.clone(), sandbox)
                .await
            {
                set_path_state(path_outcomes, path, checked_failure_state(&source));
                // A remote executor may have unlinked the file and then lost
                // the response, so no delete failure proves the disk state.
                delta.exact = false;
                return Err(checked_error("remove patch target", path, source));
            }
            set_path_state(path_outcomes, path, PreparedPatchPathState::Deleted);
            delta.changes.push(AppliedPatchChange {
                path: path.clone(),
                change: AppliedPatchFileChange::Delete {
                    content: original_content.clone(),
                },
            });
            deleted_files.push(path.clone());
            affected.deleted.push(summary_path.clone());
        }
        PreparedChange::Update {
            source,
            destination,
            content,
            old_content,
            overwritten_move_content,
            summary_path,
        } => {
            let source_snapshot = snapshot_for_resolved(snapshots, source)?;
            let target = destination.as_ref().unwrap_or(source);
            let target_snapshot = snapshot_for_resolved(snapshots, target)?;
            if let Err(source_error) = fs
                .write_file_checked(
                    target,
                    content.as_bytes().to_vec(),
                    target_snapshot.expected.clone(),
                    sandbox,
                )
                .await
            {
                set_path_state(path_outcomes, target, checked_failure_state(&source_error));
                delta.exact = false;
                return Err(checked_error("write patch target", target, source_error));
            }
            set_path_state(
                path_outcomes,
                target,
                PreparedPatchPathState::Written {
                    content: content.clone(),
                },
            );

            if let Some(destination) = destination {
                let provisional_index = delta.changes.len();
                delta.changes.push(AppliedPatchChange {
                    path: destination.clone(),
                    change: AppliedPatchFileChange::Add {
                        content: content.clone(),
                        overwritten_content: overwritten_move_content.clone(),
                    },
                });
                written_files.push((destination.clone(), content.clone()));
                if let Err(source_error) = fs
                    .remove_file_checked(source, source_snapshot.expected.clone(), sandbox)
                    .await
                {
                    set_path_state(path_outcomes, source, checked_failure_state(&source_error));
                    // The destination write is known, but the source unlink
                    // may also have reached the executor before transport loss.
                    delta.exact = false;
                    return Err(checked_error(
                        "remove original patch target",
                        source,
                        source_error,
                    ));
                }
                set_path_state(path_outcomes, source, PreparedPatchPathState::Deleted);
                delta.changes[provisional_index] = AppliedPatchChange {
                    path: source.clone(),
                    change: AppliedPatchFileChange::Update {
                        move_path: Some(destination.clone()),
                        old_content: old_content.clone(),
                        overwritten_move_content: overwritten_move_content.clone(),
                        new_content: content.clone(),
                    },
                };
                deleted_files.push(source.clone());
            } else {
                delta.changes.push(AppliedPatchChange {
                    path: source.clone(),
                    change: AppliedPatchFileChange::Update {
                        move_path: None,
                        old_content: old_content.clone(),
                        overwritten_move_content: None,
                        new_content: content.clone(),
                    },
                });
                written_files.push((source.clone(), content.clone()));
            }
            affected.modified.push(summary_path.clone());
        }
    }
    Ok(())
}

fn checked_failure_state(error: &io::Error) -> PreparedPatchPathState {
    if is_file_mutation_conflict(error) {
        PreparedPatchPathState::StaleExternal
    } else {
        PreparedPatchPathState::Unknown
    }
}

fn set_path_state(
    outcomes: &mut [PreparedPatchPathOutcome],
    path: &PathUri,
    state: PreparedPatchPathState,
) {
    if let Some(outcome) = outcomes.iter_mut().find(|outcome| &outcome.path == path) {
        outcome.state = state;
    }
}

fn snapshot_text(snapshot: &PathSnapshot) -> Result<Option<String>, PreparedPatchError> {
    snapshot
        .original
        .as_ref()
        .map(|bytes| {
            String::from_utf8(bytes.clone())
                .map_err(|error| non_utf8_target(&snapshot.logical_path, error))
        })
        .transpose()
}

fn non_utf8_target(path: &PathUri, error: std::string::FromUtf8Error) -> PreparedPatchError {
    PreparedPatchError::InvalidTarget(format!(
        "patch target is not UTF-8 text {}: {error}",
        bounded_path(path)
    ))
}

fn checked_error(operation: &'static str, path: &PathUri, source: io::Error) -> PreparedPatchError {
    if is_file_mutation_conflict(&source) {
        PreparedPatchError::StaleTarget(bounded_path(path))
    } else {
        io_error(operation, path, source)
    }
}

fn io_error(operation: &'static str, path: &PathUri, source: io::Error) -> PreparedPatchError {
    PreparedPatchError::Io {
        operation,
        path: bounded_path(path),
        source,
    }
}

#[cfg(test)]
#[path = "prepared_patch.test.rs"]
mod tests;
