mod file_update;
mod invocation;
mod parser;
mod path_effects;
mod prepared_patch;
mod seek_sequence;
mod standalone_executable;
mod streaming_parser;
mod text_file;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use coco_exec_server::CreateDirectoryOptions;
use coco_exec_server::ExecutorFileSystem;
use coco_exec_server::FileSystemSandboxContext;
use coco_exec_server::RemoveOptions;
use coco_utils_path_uri::PathUri;
use coco_utils_path_uri::PathUriParseError;
pub use parser::Hunk;
pub use parser::ParseError;
use parser::ParseError::*;
pub use parser::UpdateFileChunk;
pub use parser::parse_patch;
pub use streaming_parser::StreamingPatchParser;
use thiserror::Error;

use file_update::AppliedPatch;
pub use file_update::ApplyPatchFileUpdate;
use file_update::derive_new_contents_from_chunks;
pub use file_update::unified_diff_from_chunks;
pub use file_update::unified_diff_from_chunks_with_context;
pub(crate) use file_update::unified_diff_from_chunks_with_mode;
pub use invocation::MaybeApplyPatch;
pub use invocation::maybe_parse_apply_patch;
pub use invocation::maybe_parse_apply_patch_verified;
pub use invocation::maybe_parse_apply_patch_verified_with_mode;
pub use invocation::verify_apply_patch_args;
pub use invocation::verify_apply_patch_args_with_mode;
pub use path_effects::ApplyPatchPathEffects;
pub use path_effects::collect_path_effects;
pub use path_effects::validate_hunk_paths;
pub use prepared_patch::CommittedPatch;
pub use prepared_patch::PreparedPatch;
pub use prepared_patch::PreparedPatchCommitFailure;
pub use prepared_patch::PreparedPatchError;
pub use prepared_patch::PreparedPatchPaths;
pub use prepared_patch::commit_prepared_patch;
pub use prepared_patch::prepare_hunk_paths;
pub use prepared_patch::prepare_hunks;
pub use prepared_patch::prepare_hunks_from_paths;
pub use standalone_executable::main;

use crate::invocation::ExtractHeredocError;

/// Internal environment variable used to carry the selected update mode
/// into the standalone executable.
pub const COCO_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR: &str =
    "COCO_APPLY_PATCH_PRESERVE_LINE_ENDINGS";

/// Controls how updates reconstruct the target file after matching a patch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApplyPatchFileUpdateMode {
    /// Preserve the historical behavior of normalizing updated files to LF.
    #[default]
    NormalizeToLf,
    /// Preserve existing line endings and use the file's preferred ending for new lines.
    PreserveLineEndings,
}

/// Reads the update mode selected for a standalone `apply_patch` process.
#[doc(hidden)]
pub fn apply_patch_file_update_mode_from_env() -> ApplyPatchFileUpdateMode {
    match std::env::var(COCO_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR).as_deref() {
        Ok("1") => ApplyPatchFileUpdateMode::PreserveLineEndings,
        _ => ApplyPatchFileUpdateMode::NormalizeToLf,
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ApplyPatchError {
    #[error(transparent)]
    ParseError(#[from] ParseError),
    #[error(transparent)]
    IoError(#[from] IoError),
    /// Error that occurs while computing replacements when applying patch chunks
    #[error("{0}")]
    ComputeReplacements(String),
    /// A patch path could not be resolved as a path URI.
    #[error(transparent)]
    PathUri(#[from] PathUriParseError),
    /// A raw patch body was provided without an explicit `apply_patch` invocation.
    #[error(
        "patch detected without explicit call to apply_patch. Rerun as [\"apply_patch\", \"<patch>\"]"
    )]
    ImplicitInvocation,
}

impl From<std::io::Error> for ApplyPatchError {
    fn from(err: std::io::Error) -> Self {
        ApplyPatchError::IoError(IoError {
            context: "I/O error".to_string(),
            source: err,
        })
    }
}

impl From<&std::io::Error> for ApplyPatchError {
    fn from(err: &std::io::Error) -> Self {
        ApplyPatchError::IoError(IoError {
            context: "I/O error".to_string(),
            source: std::io::Error::new(err.kind(), err.to_string()),
        })
    }
}

#[derive(Debug, Error)]
#[error("{context}: {source}")]
pub struct IoError {
    context: String,
    #[source]
    source: std::io::Error,
}

impl PartialEq for IoError {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context && self.source.to_string() == other.source.to_string()
    }
}

/// Both the raw PATCH argument to `apply_patch` as well as the PATCH argument
/// parsed into hunks.
#[derive(Debug, PartialEq)]
pub struct ApplyPatchArgs {
    pub patch: String,
    pub hunks: Vec<Hunk>,
    pub workdir: Option<String>,
    pub environment_id: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ApplyPatchFileChange {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        move_path: Option<PathUri>,
        /// new_content that will result after the unified_diff is applied.
        new_content: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum MaybeApplyPatchVerified {
    /// `argv` corresponded to an `apply_patch` invocation, and these are the
    /// resulting proposed file changes.
    Body(ApplyPatchAction),
    /// `argv` could not be parsed to determine whether it corresponds to an
    /// `apply_patch` invocation.
    ShellParseError(ExtractHeredocError),
    /// `argv` corresponded to an `apply_patch` invocation, but it could not
    /// be fulfilled due to the specified error.
    CorrectnessError(ApplyPatchError),
    /// `argv` decidedly did not correspond to an `apply_patch` invocation.
    NotApplyPatch,
}

/// ApplyPatchAction is the result of parsing an `apply_patch` command. By
/// construction, all paths should be absolute paths.
#[derive(Debug, PartialEq)]
pub struct ApplyPatchAction {
    changes: HashMap<PathUri, ApplyPatchFileChange>,

    update_file_mode: ApplyPatchFileUpdateMode,

    /// The raw patch argument that can be used to apply the patch. i.e., if the
    /// original arg was parsed in "lenient" mode with a
    /// heredoc, this should be the value without the heredoc wrapper.
    pub patch: String,

    /// The working directory that was used to resolve relative paths in the patch.
    pub cwd: PathUri,
}

impl ApplyPatchAction {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Returns the changes that would be made by applying the patch.
    pub fn changes(&self) -> &HashMap<PathUri, ApplyPatchFileChange> {
        &self.changes
    }

    /// Returns the update mode selected while the patch was verified.
    pub fn update_file_mode(&self) -> ApplyPatchFileUpdateMode {
        self.update_file_mode
    }

    /// Should be used exclusively for testing. (Not worth the overhead of
    /// creating a feature flag for this.)
    pub fn new_add_for_test(path: &PathUri, content: String) -> Self {
        #[expect(clippy::expect_used)]
        let filename = path.basename().expect("path should not be empty");
        let patch = format!(
            r#"*** Begin Patch
*** Update File: {filename}
@@
+ {content}
*** End Patch"#,
        );
        let changes = HashMap::from([(path.clone(), ApplyPatchFileChange::Add { content })]);
        #[expect(clippy::expect_used)]
        Self {
            changes,
            update_file_mode: ApplyPatchFileUpdateMode::default(),
            cwd: path.parent().expect("path should have parent"),
            patch,
        }
    }
}

/// Textual file changes that were actually committed while applying a patch.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedPatchDelta {
    changes: Vec<AppliedPatchChange>,
    exact: bool,
}

impl AppliedPatchDelta {
    fn new(changes: Vec<AppliedPatchChange>, exact: bool) -> Self {
        Self { changes, exact }
    }

    fn empty() -> Self {
        Self::new(Vec::new(), /*exact*/ true)
    }

    pub fn changes(&self) -> &[AppliedPatchChange] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn is_exact(&self) -> bool {
        self.exact
    }

    /// Appends a later committed prefix while preserving the aggregate exactness.
    pub fn append(&mut self, other: Self) {
        self.changes.extend(other.changes);
        self.exact &= other.exact;
    }
}

impl Default for AppliedPatchDelta {
    fn default() -> Self {
        Self::empty()
    }
}

/// A committed file change, preserved in the order it was applied.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedPatchChange {
    pub path: PathUri,
    pub change: AppliedPatchFileChange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppliedPatchFileChange {
    Add {
        content: String,
        overwritten_content: Option<String>,
    },
    Delete {
        content: String,
    },
    Update {
        move_path: Option<PathUri>,
        old_content: String,
        overwritten_move_content: Option<String>,
        new_content: String,
    },
}

/// A failed patch application together with the textual mutations that were
/// definitely committed before the failure was observed.
#[derive(Debug, Error)]
#[error("{error}")]
pub struct ApplyPatchFailure {
    #[source]
    error: ApplyPatchError,
    delta: AppliedPatchDelta,
}

impl ApplyPatchFailure {
    fn new(error: ApplyPatchError, delta: AppliedPatchDelta) -> Self {
        Self { error, delta }
    }

    fn without_delta(error: ApplyPatchError) -> Self {
        Self::new(error, AppliedPatchDelta::empty())
    }

    pub fn delta(&self) -> &AppliedPatchDelta {
        &self.delta
    }

    pub fn into_parts(self) -> (ApplyPatchError, AppliedPatchDelta) {
        (self.error, self.delta)
    }
}

/// Applies the patch and prints the result to stdout/stderr.
pub async fn apply_patch(
    patch: &str,
    cwd: &PathUri,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    apply_patch_with_mode(
        patch,
        ApplyPatchFileUpdateMode::default(),
        cwd,
        stdout,
        stderr,
        fs,
        sandbox,
    )
    .await
}

/// Applies the patch using the selected file-update mode and prints the result
/// to stdout/stderr.
pub async fn apply_patch_with_mode(
    patch: &str,
    update_file_mode: ApplyPatchFileUpdateMode,
    cwd: &PathUri,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let hunks = match parse_patch(patch) {
        Ok(source) if source.environment_id.is_none() => source.hunks,
        Ok(_) => {
            let message =
                "apply_patch environment selection is unavailable in this execution context";
            writeln!(stderr, "Invalid patch: {message}")
                .map_err(ApplyPatchError::from)
                .map_err(ApplyPatchFailure::without_delta)?;
            return Err(ApplyPatchFailure::without_delta(
                ApplyPatchError::ParseError(InvalidPatchError(message.to_string())),
            ));
        }
        Err(e) => {
            match &e {
                InvalidPatchError(message) => {
                    writeln!(stderr, "Invalid patch: {message}")
                        .map_err(ApplyPatchError::from)
                        .map_err(ApplyPatchFailure::without_delta)?;
                }
                InvalidHunkError {
                    message,
                    line_number,
                } => {
                    writeln!(
                        stderr,
                        "Invalid patch hunk on line {line_number}: {message}"
                    )
                    .map_err(ApplyPatchError::from)
                    .map_err(ApplyPatchFailure::without_delta)?;
                }
            }
            return Err(ApplyPatchFailure::without_delta(
                ApplyPatchError::ParseError(e),
            ));
        }
    };

    apply_hunks_with_mode(&hunks, update_file_mode, cwd, stdout, stderr, fs, sandbox).await
}

/// Applies hunks and continues to update stdout/stderr
pub async fn apply_hunks(
    hunks: &[Hunk],
    cwd: &PathUri,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    apply_hunks_with_mode(
        hunks,
        ApplyPatchFileUpdateMode::default(),
        cwd,
        stdout,
        stderr,
        fs,
        sandbox,
    )
    .await
}

/// Applies hunks using the selected file-update mode and continues to update
/// stdout/stderr.
async fn apply_hunks_with_mode(
    hunks: &[Hunk],
    update_file_mode: ApplyPatchFileUpdateMode,
    cwd: &PathUri,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let mut delta = AppliedPatchDelta::empty();
    match apply_hunks_to_files(hunks, update_file_mode, cwd, fs, sandbox, &mut delta).await {
        Ok(affected_paths) => {
            print_summary(&affected_paths, stdout).map_err(|error| {
                ApplyPatchFailure::new(ApplyPatchError::from(error), delta.clone())
            })?;
            Ok(delta)
        }
        Err(error) => {
            let msg = error.to_string();
            writeln!(stderr, "{msg}").map_err(|error| {
                ApplyPatchFailure::new(ApplyPatchError::from(error), delta.clone())
            })?;
            let error = if let Some(io) = error.downcast_ref::<std::io::Error>() {
                ApplyPatchError::from(io)
            } else {
                ApplyPatchError::IoError(IoError {
                    context: msg,
                    source: std::io::Error::other(error),
                })
            };
            Err(ApplyPatchFailure::new(error, delta))
        }
    }
}

/// Applies each parsed patch hunk to the filesystem.
/// Returns an error if any of the changes could not be applied.
/// Tracks file paths affected by applying a patch, preserving the path spelling
/// from the patch for user-facing summaries.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct AffectedPaths {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Apply the hunks to the filesystem, returning which files were added, modified, or deleted.
/// Returns an error if the patch could not be applied.
async fn apply_hunks_to_files(
    hunks: &[Hunk],
    update_file_mode: ApplyPatchFileUpdateMode,
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
    delta: &mut AppliedPatchDelta,
) -> anyhow::Result<AffectedPaths> {
    if hunks.is_empty() {
        anyhow::bail!("No files were modified.");
    }

    let mut added: Vec<PathBuf> = Vec::new();
    let mut modified: Vec<PathBuf> = Vec::new();
    let mut deleted: Vec<PathBuf> = Vec::new();
    // A failed write can still have modified the target before surfacing an
    // error (for example by truncating before ENOSPC), so the accumulated
    // delta is no longer exact when a write fails.
    macro_rules! try_write {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    delta.exact = false;
                    return Err(anyhow::Error::from(error));
                }
            }
        };
    }

    for hunk in hunks {
        let affected_path = hunk.path().to_path_buf();
        let path_uri = hunk.resolve_path(cwd)?;
        match hunk {
            Hunk::AddFile { contents, .. } => {
                let overwritten_content =
                    read_optional_file_text_for_delta(&path_uri, fs, sandbox, &mut delta.exact)
                        .await;
                try_write!(
                    write_file_with_missing_parent_retry(
                        fs,
                        &path_uri,
                        contents.clone().into_bytes(),
                        sandbox,
                    )
                    .await
                );
                delta.changes.push(AppliedPatchChange {
                    path: path_uri,
                    change: AppliedPatchFileChange::Add {
                        content: contents.clone(),
                        overwritten_content,
                    },
                });
                added.push(affected_path);
            }
            Hunk::DeleteFile { .. } => {
                note_existing_path_delta_support(&path_uri, fs, sandbox, &mut delta.exact).await;
                let deleted_content = fs.read_file_text(&path_uri, sandbox).await.ok();
                if deleted_content.is_none() {
                    delta.exact = false;
                }
                ensure_not_directory(&path_uri, fs, sandbox)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to delete file {}",
                            path_uri.inferred_native_path_string()
                        )
                    })?;
                if let Err(error) = fs
                    .remove(
                        &path_uri,
                        RemoveOptions {
                            recursive: false,
                            force: false,
                        },
                        sandbox,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to delete file {}",
                            path_uri.inferred_native_path_string()
                        )
                    })
                {
                    delta.exact &= remove_failure_was_side_effect_free(
                        &path_uri,
                        deleted_content.as_deref(),
                        fs,
                        sandbox,
                    )
                    .await;
                    return Err(error);
                }
                if let Some(content) = deleted_content {
                    delta.changes.push(AppliedPatchChange {
                        path: path_uri,
                        change: AppliedPatchFileChange::Delete { content },
                    });
                }
                deleted.push(affected_path);
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                note_existing_path_delta_support(&path_uri, fs, sandbox, &mut delta.exact).await;
                let AppliedPatch {
                    original_contents,
                    new_contents,
                } = derive_new_contents_from_chunks(
                    &path_uri,
                    chunks,
                    update_file_mode,
                    fs,
                    sandbox,
                )
                .await?;
                if let Some(dest) = move_path {
                    let dest_uri = cwd.join(&dest.to_string_lossy())?;
                    let overwritten_move_content =
                        read_optional_file_text_for_delta(&dest_uri, fs, sandbox, &mut delta.exact)
                            .await;
                    try_write!(
                        write_file_with_missing_parent_retry(
                            fs,
                            &dest_uri,
                            new_contents.clone().into_bytes(),
                            sandbox,
                        )
                        .await
                    );
                    let dest_write_change_index = delta.changes.len();
                    delta.changes.push(AppliedPatchChange {
                        path: dest_uri.clone(),
                        change: AppliedPatchFileChange::Add {
                            content: new_contents.clone(),
                            overwritten_content: overwritten_move_content.clone(),
                        },
                    });
                    ensure_not_directory(&path_uri, fs, sandbox)
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to remove original {}",
                                path_uri.inferred_native_path_string()
                            )
                        })?;
                    if let Err(error) = fs
                        .remove(
                            &path_uri,
                            RemoveOptions {
                                recursive: false,
                                force: false,
                            },
                            sandbox,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to remove original {}",
                                path_uri.inferred_native_path_string()
                            )
                        })
                    {
                        delta.exact &= remove_failure_was_side_effect_free(
                            &path_uri,
                            Some(&original_contents),
                            fs,
                            sandbox,
                        )
                        .await;
                        return Err(error);
                    }
                    delta.changes[dest_write_change_index] = AppliedPatchChange {
                        path: path_uri,
                        change: AppliedPatchFileChange::Update {
                            move_path: Some(dest_uri),
                            old_content: original_contents,
                            overwritten_move_content,
                            new_content: new_contents,
                        },
                    };
                    modified.push(affected_path);
                } else {
                    try_write!(
                        fs.write_file(&path_uri, new_contents.clone().into_bytes(), sandbox)
                            .await
                            .with_context(|| format!(
                                "Failed to write file {}",
                                path_uri.inferred_native_path_string()
                            ))
                    );
                    delta.changes.push(AppliedPatchChange {
                        path: path_uri,
                        change: AppliedPatchFileChange::Update {
                            move_path: None,
                            old_content: original_contents,
                            overwritten_move_content: None,
                            new_content: new_contents,
                        },
                    });
                    modified.push(affected_path);
                }
            }
        }
    }
    Ok(AffectedPaths {
        added,
        modified,
        deleted,
    })
}

async fn ensure_not_directory(
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> io::Result<()> {
    let metadata = fs.get_metadata(path, sandbox).await?;
    if metadata.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is a directory",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;

async fn remove_failure_was_side_effect_free(
    path: &PathUri,
    expected_content: Option<&str>,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> bool {
    match expected_content {
        Some(expected_content) => fs
            .read_file_text(path, sandbox)
            .await
            .is_ok_and(|content| content == expected_content),
        None => false,
    }
}

async fn read_optional_file_text_for_delta(
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
    exact: &mut bool,
) -> Option<String> {
    note_existing_path_delta_support(path, fs, sandbox, exact).await;
    match fs.read_file_text(path, sandbox).await {
        Ok(content) => Some(content),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(_) => {
            *exact = false;
            None
        }
    }
}

async fn note_existing_path_delta_support(
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
    exact: &mut bool,
) {
    match fs.get_metadata(path, sandbox).await {
        Ok(metadata) if metadata.is_file && !metadata.is_symlink => {}
        Ok(_) => *exact = false,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(_) => *exact = false,
    }
}

async fn write_file_with_missing_parent_retry(
    fs: &dyn ExecutorFileSystem,
    path: &PathUri,
    contents: Vec<u8>,
    sandbox: Option<&FileSystemSandboxContext>,
) -> anyhow::Result<()> {
    match fs.write_file(path, contents.clone(), sandbox).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                fs.create_directory(&parent, CreateDirectoryOptions { recursive: true }, sandbox)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to create parent directories for {}",
                            path.inferred_native_path_string()
                        )
                    })?;
            }
            fs.write_file(path, contents, sandbox)
                .await
                .with_context(|| {
                    format!(
                        "Failed to write file {}",
                        path.inferred_native_path_string()
                    )
                })?;
            Ok(())
        }
        Err(err) => Err(err).with_context(|| {
            format!(
                "Failed to write file {}",
                path.inferred_native_path_string()
            )
        }),
    }
}

/// Print the summary of changes in git-style format.
/// Write a summary of changes to the given writer.
pub fn print_summary(
    affected: &AffectedPaths,
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    writeln!(out, "Success. Updated the following files:")?;
    for path in &affected.added {
        writeln!(out, "A {}", path.display())?;
    }
    for path in &affected.modified {
        writeln!(out, "M {}", path.display())?;
    }
    for path in &affected.deleted {
        writeln!(out, "D {}", path.display())?;
    }
    Ok(())
}
