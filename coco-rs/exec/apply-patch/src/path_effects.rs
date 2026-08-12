use std::collections::HashSet;
use std::io;

use coco_exec_server::ExecutorFileSystem;
use coco_exec_server::FileSystemSandboxContext;
use coco_utils_path_uri::PathUri;

use crate::ApplyPatchError;
use crate::Hunk;
use crate::IoError;
use crate::ParseError;

const MAX_ERROR_PATH_CHARS: usize = 512;

/// Unique paths affected by a patch, in operation order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplyPatchPathEffects {
    paths: Vec<PathUri>,
    logical_paths: Vec<PathUri>,
}

impl ApplyPatchPathEffects {
    /// Returns the unique source and move-destination paths in operation order.
    pub fn paths(&self) -> &[PathUri] {
        &self.paths
    }

    /// Returns the patch-spelled paths before canonicalization. These are used
    /// only for path-triggered features; authorization and mutation use
    /// [`Self::paths`].
    pub fn logical_paths(&self) -> &[PathUri] {
        &self.logical_paths
    }

    /// Consumes the path set and returns its ordered paths.
    pub fn into_paths(self) -> Vec<PathUri> {
        self.paths
    }
}

/// Resolves every source and move destination without accessing the filesystem.
pub fn collect_path_effects(
    hunks: &[Hunk],
    cwd: &PathUri,
) -> Result<ApplyPatchPathEffects, ApplyPatchError> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for path in resolve_operation_paths(hunks, cwd)? {
        push_unique(&mut paths, &mut seen, path);
    }

    Ok(ApplyPatchPathEffects {
        logical_paths: paths.clone(),
        paths,
    })
}

/// Rejects patches whose source or move-destination paths resolve to the same
/// filesystem object. Existing ancestors are canonicalized so aliases through
/// symlinks cannot bypass the duplicate-path check.
pub async fn validate_hunk_paths(
    hunks: &[Hunk],
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<ApplyPatchPathEffects, ApplyPatchError> {
    Ok(resolve_and_validate_hunk_paths(hunks, cwd, fs, sandbox)
        .await?
        .path_effects)
}

pub(crate) struct ValidatedHunkPaths {
    pub(crate) operations: Vec<ResolvedOperationPath>,
    pub(crate) path_effects: ApplyPatchPathEffects,
}

pub(crate) struct ResolvedOperationPath {
    pub(crate) logical_path: PathUri,
    pub(crate) resolved: ResolvedPathIdentity,
}

pub(crate) async fn resolve_and_validate_hunk_paths(
    hunks: &[Hunk],
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<ValidatedHunkPaths, ApplyPatchError> {
    let paths = resolve_operation_paths(hunks, cwd)?;
    let mut operations = Vec::with_capacity(paths.len());
    let mut identities = HashSet::new();

    for path in paths {
        let resolved = resolve_path_identity(&path, fs, sandbox).await?;
        if !identities.insert(resolved.identity.clone()) {
            return Err(ParseError::InvalidPatchError(format!(
                "multiple operations target {}",
                bounded_path(&path)
            ))
            .into());
        }
        operations.push(ResolvedOperationPath {
            logical_path: path,
            resolved,
        });
    }

    let logical_paths = operations
        .iter()
        .map(|operation| operation.logical_path.clone())
        .collect::<Vec<_>>();
    let resolved_paths = operations
        .iter()
        .map(|operation| operation.resolved.path.clone())
        .collect::<Vec<_>>();
    let path_effects = ApplyPatchPathEffects {
        paths: resolved_paths,
        logical_paths,
    };
    Ok(ValidatedHunkPaths {
        operations,
        path_effects,
    })
}

fn resolve_operation_paths(hunks: &[Hunk], cwd: &PathUri) -> Result<Vec<PathUri>, ApplyPatchError> {
    let mut paths = Vec::new();
    for hunk in hunks {
        paths.push(hunk.resolve_path(cwd)?);
        if let Hunk::UpdateFile {
            move_path: Some(move_path),
            ..
        } = hunk
        {
            paths.push(cwd.join(&move_path.to_string_lossy())?);
        }
    }
    Ok(paths)
}

fn push_unique(paths: &mut Vec<PathUri>, seen: &mut HashSet<PathUri>, path: PathUri) {
    if seen.insert(path.clone()) {
        paths.push(path);
    }
}

pub(crate) struct ResolvedPathIdentity {
    pub(crate) path: PathUri,
    pub(crate) identity: String,
}

pub(crate) async fn resolve_path_identity(
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<ResolvedPathIdentity, ApplyPatchError> {
    let mut candidate = path.clone();
    let mut missing_segments = Vec::new();

    let canonical = loop {
        match fs.canonicalize(&candidate, sandbox).await {
            Ok(canonical) => break canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = candidate.parent() else {
                    return Err(path_io_error("Failed to resolve patch path", path, error));
                };
                let Some(segment) = candidate.basename() else {
                    return Err(path_io_error("Failed to resolve patch path", path, error));
                };
                missing_segments.push(segment);
                candidate = parent;
            }
            Err(error) => {
                return Err(path_io_error("Failed to resolve patch path", path, error));
            }
        }
    };

    let canonical = missing_segments
        .iter()
        .rev()
        .try_fold(canonical, |parent, segment| parent.join(segment))?;
    let identity = fs.path_comparison_key(&canonical);
    Ok(ResolvedPathIdentity {
        path: canonical,
        identity,
    })
}

fn path_io_error(context: &str, path: &PathUri, source: io::Error) -> ApplyPatchError {
    ApplyPatchError::IoError(IoError {
        context: format!("{context} {}", bounded_path(path)),
        source,
    })
}

pub(crate) fn bounded_path(path: &PathUri) -> String {
    let path = path.inferred_native_path_string();
    if path.chars().count() <= MAX_ERROR_PATH_CHARS {
        return path;
    }

    let mut bounded = path
        .chars()
        .take(MAX_ERROR_PATH_CHARS.saturating_sub(1))
        .collect::<String>();
    bounded.push('…');
    bounded
}

#[cfg(test)]
#[path = "path_effects.test.rs"]
mod tests;
