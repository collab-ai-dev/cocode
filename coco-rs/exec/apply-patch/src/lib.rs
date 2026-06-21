mod invocation;
mod parser;
mod seek_sequence;
mod standalone_executable;

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use coco_exec_server::CreateDirectoryOptions;
use coco_exec_server::ExecutorFileSystem;
use coco_exec_server::RemoveOptions;
use coco_utils_absolute_path::AbsolutePathBuf;
pub use parser::Hunk;
pub use parser::ParseError;
use parser::ParseError::*;
use parser::UpdateFileChunk;
pub use parser::parse_patch;
use similar::TextDiff;
use thiserror::Error;

pub use invocation::maybe_parse_apply_patch_verified;
pub use standalone_executable::main;

use crate::invocation::ExtractHeredocError;

/// Detailed instructions for gpt-4.1 on how to use the `apply_patch` tool.
pub const APPLY_PATCH_TOOL_INSTRUCTIONS: &str = include_str!("../apply_patch_tool_instructions.md");

/// Special argv[1] flag used when the Codex executable self-invokes to run the
/// internal `apply_patch` path.
///
/// Although this constant lives in `codex-apply-patch` (to avoid forcing
/// `codex-arg0` to depend on `codex-core`), it is part of the "codex core"
/// process-invocation contract between the apply-patch runtime and the arg0
/// dispatcher.
pub const COCO_CORE_APPLY_PATCH_ARG1: &str = "--coco-run-as-apply-patch";

#[derive(Debug, Error, PartialEq)]
pub enum ApplyPatchError {
    #[error(transparent)]
    ParseError(#[from] ParseError),
    #[error(transparent)]
    IoError(#[from] IoError),
    /// Error that occurs while computing replacements when applying patch chunks
    #[error("{0}")]
    ComputeReplacements(String),
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
        move_path: Option<PathBuf>,
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
    changes: HashMap<PathBuf, ApplyPatchFileChange>,

    /// The raw patch argument that can be used with `apply_patch` as an exec
    /// call. i.e., if the original arg was parsed in "lenient" mode with a
    /// heredoc, this should be the value without the heredoc wrapper.
    pub patch: String,

    /// The working directory that was used to resolve relative paths in the patch.
    pub cwd: AbsolutePathBuf,
}

impl ApplyPatchAction {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Returns the changes that would be made by applying the patch.
    pub fn changes(&self) -> &HashMap<PathBuf, ApplyPatchFileChange> {
        &self.changes
    }

    /// Should be used exclusively for testing. (Not worth the overhead of
    /// creating a feature flag for this.)
    pub fn new_add_for_test(path: &AbsolutePathBuf, content: String) -> Self {
        #[expect(clippy::expect_used)]
        let filename = path
            .file_name()
            .expect("path should not be empty")
            .to_string_lossy();
        let patch = format!(
            r#"*** Begin Patch
*** Update File: {filename}
@@
+ {content}
*** End Patch"#,
        );
        let changes = HashMap::from([(path.to_path_buf(), ApplyPatchFileChange::Add { content })]);
        #[expect(clippy::expect_used)]
        Self {
            changes,
            cwd: path.parent().expect("path should have parent"),
            patch,
        }
    }
}

/// Absolute paths affected by parsed apply-patch hunks, grouped by consumer.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ApplyPatchPathEffects {
    /// Paths that require write permission before applying the patch.
    pub permission_paths: Vec<PathBuf>,
    /// Paths whose pre-mutation state should be captured for file history.
    pub history_paths: Vec<PathBuf>,
    /// Paths to notify LSP about after a successful patch.
    pub lsp_notify_paths: Vec<PathBuf>,
}

/// Collect path effects from parsed hunks using the same move/source semantics
/// as the patch applier.
pub fn collect_path_effects(hunks: &[Hunk], cwd: &AbsolutePathBuf) -> ApplyPatchPathEffects {
    let mut permission_paths = BTreeSet::new();
    let mut history_paths = BTreeSet::new();
    let mut lsp_notify_paths = BTreeSet::new();

    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, .. } => {
                let target = resolve_patch_path(path, cwd);
                permission_paths.insert(target.clone());
                history_paths.insert(target.clone());
                lsp_notify_paths.insert(target);
            }
            Hunk::DeleteFile { path } => {
                let source = resolve_patch_path(path, cwd);
                permission_paths.insert(source.clone());
                history_paths.insert(source.clone());
                lsp_notify_paths.insert(source);
            }
            Hunk::UpdateFile {
                path, move_path, ..
            } => {
                let source = resolve_patch_path(path, cwd);
                permission_paths.insert(source.clone());
                history_paths.insert(source.clone());
                lsp_notify_paths.insert(source);

                if let Some(move_path) = move_path {
                    let destination = resolve_patch_path(move_path, cwd);
                    permission_paths.insert(destination.clone());
                    history_paths.insert(destination.clone());
                    lsp_notify_paths.insert(destination);
                }
            }
        }
    }

    ApplyPatchPathEffects {
        permission_paths: permission_paths.into_iter().collect(),
        history_paths: history_paths.into_iter().collect(),
        lsp_notify_paths: lsp_notify_paths.into_iter().collect(),
    }
}

fn resolve_patch_path(path: &Path, cwd: &AbsolutePathBuf) -> PathBuf {
    AbsolutePathBuf::resolve_path_against_base(path, cwd)
        .as_path()
        .to_path_buf()
}

/// Applies the patch and prints the result to stdout/stderr.
pub async fn apply_patch(
    patch: &str,
    cwd: &AbsolutePathBuf,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
) -> Result<(), ApplyPatchError> {
    let hunks = match parse_patch(patch) {
        Ok(source) => source.hunks,
        Err(e) => {
            match &e {
                InvalidPatchError(message) => {
                    writeln!(stderr, "Invalid patch: {message}").map_err(ApplyPatchError::from)?;
                }
                InvalidHunkError {
                    message,
                    line_number,
                } => {
                    writeln!(
                        stderr,
                        "Invalid patch hunk on line {line_number}: {message}"
                    )
                    .map_err(ApplyPatchError::from)?;
                }
            }
            return Err(ApplyPatchError::ParseError(e));
        }
    };

    apply_hunks(&hunks, cwd, stdout, stderr, fs).await?;

    Ok(())
}

/// Applies hunks and continues to update stdout/stderr
pub async fn apply_hunks(
    hunks: &[Hunk],
    cwd: &AbsolutePathBuf,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
    fs: &dyn ExecutorFileSystem,
) -> Result<(), ApplyPatchError> {
    // Delegate to a helper that applies each hunk to the filesystem.
    match apply_hunks_to_files(hunks, cwd, fs).await {
        Ok(affected) => {
            print_summary(&affected, stdout).map_err(ApplyPatchError::from)?;
            Ok(())
        }
        Err(err) => {
            let msg = err.to_string();
            writeln!(stderr, "{msg}").map_err(ApplyPatchError::from)?;
            if let Some(io) = err.downcast_ref::<std::io::Error>() {
                Err(ApplyPatchError::from(io))
            } else {
                Err(ApplyPatchError::IoError(IoError {
                    context: msg,
                    source: std::io::Error::other(err),
                }))
            }
        }
    }
}

/// Applies each parsed patch hunk to the filesystem.
/// Returns an error if any of the changes could not be applied.
/// Tracks file paths affected by applying a patch, preserving the path spelling
/// from the patch for user-facing summaries.
pub struct AffectedPaths {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Apply the hunks to the filesystem, returning which files were added, modified, or deleted.
/// Returns an error if the patch could not be applied.
async fn apply_hunks_to_files(
    hunks: &[Hunk],
    cwd: &AbsolutePathBuf,
    fs: &dyn ExecutorFileSystem,
) -> anyhow::Result<AffectedPaths> {
    if hunks.is_empty() {
        anyhow::bail!("No files were modified.");
    }

    let mut added: Vec<PathBuf> = Vec::new();
    let mut modified: Vec<PathBuf> = Vec::new();
    let mut deleted: Vec<PathBuf> = Vec::new();
    for hunk in hunks {
        let affected_path = hunk.path().to_path_buf();
        let path_abs = hunk.resolve_path(cwd);
        match hunk {
            Hunk::AddFile { contents, .. } => {
                if let Some(parent_abs) = path_abs.parent() {
                    fs.create_directory(&parent_abs, CreateDirectoryOptions { recursive: true })
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to create parent directories for {}",
                                path_abs.display()
                            )
                        })?;
                }
                fs.write_file(&path_abs, contents.clone().into_bytes())
                    .await
                    .with_context(|| format!("Failed to write file {}", path_abs.display()))?;
                added.push(affected_path);
            }
            Hunk::DeleteFile { .. } => {
                let result: io::Result<()> = async {
                    let metadata = fs.get_metadata(&path_abs).await?;
                    if metadata.is_directory {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "path is a directory",
                        ));
                    }
                    fs.remove(
                        &path_abs,
                        RemoveOptions {
                            recursive: false,
                            force: false,
                        },
                    )
                    .await
                }
                .await;
                result.with_context(|| format!("Failed to delete file {}", path_abs.display()))?;
                deleted.push(affected_path);
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                let AppliedPatch { new_contents, .. } =
                    derive_new_contents_from_chunks(&path_abs, chunks, fs).await?;
                if let Some(dest) = move_path {
                    let dest_abs = AbsolutePathBuf::resolve_path_against_base(dest, cwd);
                    if let Some(parent_abs) = dest_abs.parent() {
                        fs.create_directory(
                            &parent_abs,
                            CreateDirectoryOptions { recursive: true },
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to create parent directories for {}",
                                dest_abs.display()
                            )
                        })?;
                    }
                    fs.write_file(&dest_abs, new_contents.into_bytes())
                        .await
                        .with_context(|| format!("Failed to write file {}", dest_abs.display()))?;
                    let result: io::Result<()> = async {
                        let metadata = fs.get_metadata(&path_abs).await?;
                        if metadata.is_directory {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "path is a directory",
                            ));
                        }
                        fs.remove(
                            &path_abs,
                            RemoveOptions {
                                recursive: false,
                                force: false,
                            },
                        )
                        .await
                    }
                    .await;
                    result.with_context(|| {
                        format!("Failed to remove original {}", path_abs.display())
                    })?;
                    modified.push(affected_path);
                } else {
                    fs.write_file(&path_abs, new_contents.into_bytes())
                        .await
                        .with_context(|| format!("Failed to write file {}", path_abs.display()))?;
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

struct AppliedPatch {
    original_contents: String,
    new_contents: String,
}

/// Return *only* the new file contents (joined into a single `String`) after
/// applying the chunks to the file at `path`.
async fn derive_new_contents_from_chunks(
    path_abs: &AbsolutePathBuf,
    chunks: &[UpdateFileChunk],
    fs: &dyn ExecutorFileSystem,
) -> std::result::Result<AppliedPatch, ApplyPatchError> {
    let original_contents = fs.read_file_text(path_abs).await.map_err(|err| {
        ApplyPatchError::IoError(IoError {
            context: format!("Failed to read file to update {}", path_abs.display()),
            source: err,
        })
    })?;

    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

    // Drop the trailing empty element that results from the final newline so
    // that line counts match the behaviour of standard `diff`.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path_abs.as_path(), chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    let mut new_lines = new_lines;
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    let new_contents = new_lines.join("\n");
    Ok(AppliedPatch {
        original_contents,
        new_contents,
    })
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines, given the patch `chunks`. Each replacement is returned as
/// `(start_index, old_len, new_lines)`.
fn compute_replacements(
    original_lines: &[String],
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // If a chunk has a `change_context`, we use seek_sequence to find it, then
        // adjust our `line_index` to continue from there.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{}' in {}",
                    ctx_line,
                    path.display()
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition (no old lines). We'll add them at the end or just
            // before the final empty line if one exists.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        // Otherwise, try to match the existing lines in the file with the old lines
        // from the chunk. If found, schedule that region for replacement.
        // Attempt to locate the `old_lines` verbatim within the file.  In many
        // real‑world diffs the last element of `old_lines` is an *empty* string
        // representing the terminating newline of the region being replaced.
        // This sentinel is not present in `original_lines` because we strip the
        // trailing empty slice emitted by `split('\n')`.  If a direct search
        // fails and the pattern ends with an empty string, retry without that
        // final element so that modifications touching the end‑of‑file can be
        // located reliably.

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found =
            seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line which represents the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }

            found = seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path.display(),
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by(|(lhs_idx, _, _), (rhs_idx, _, _)| lhs_idx.cmp(rhs_idx));

    Ok(replacements)
}

/// Apply the `(start_index, old_len, new_lines)` replacements to `original_lines`,
/// returning the modified file contents as a vector of lines.
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    // We must apply replacements in descending order so that earlier replacements
    // don't shift the positions of later ones.
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        // Remove old lines.
        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        // Insert new lines.
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

/// Intended result of a file update for apply_patch.
#[derive(Debug, Eq, PartialEq)]
pub struct ApplyPatchFileUpdate {
    unified_diff: String,
    content: String,
}

impl ApplyPatchFileUpdate {
    pub fn unified_diff(&self) -> &str {
        &self.unified_diff
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

pub async fn unified_diff_from_chunks(
    path_abs: &AbsolutePathBuf,
    chunks: &[UpdateFileChunk],
    fs: &dyn ExecutorFileSystem,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    unified_diff_from_chunks_with_context(path_abs, chunks, /*context*/ 1, fs).await
}

pub async fn unified_diff_from_chunks_with_context(
    path_abs: &AbsolutePathBuf,
    chunks: &[UpdateFileChunk],
    context: usize,
    fs: &dyn ExecutorFileSystem,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    let AppliedPatch {
        original_contents,
        new_contents,
    } = derive_new_contents_from_chunks(path_abs, chunks, fs).await?;
    let text_diff = TextDiff::from_lines(&original_contents, &new_contents);
    let unified_diff = text_diff.unified_diff().context_radius(context).to_string();
    Ok(ApplyPatchFileUpdate {
        unified_diff,
        content: new_contents,
    })
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

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
