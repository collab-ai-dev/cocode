use std::path::Path;
use std::path::PathBuf;

use crate::InvalidUtf8Snafu;
use crate::MissingSourceSnafu;
use crate::NamedWorkflowNotFoundSnafu;
use crate::ReadSourceSnafu;
use crate::Result;
use crate::SourceTooLargeSnafu;
use crate::UncPathSnafu;

pub const MAX_WORKFLOW_SOURCE_BYTES: usize = 512 * 1024;

const CLAUDE_CONFIG_DIR: &str = ".claude";
const WORKFLOW_SUBDIR: &str = "workflows";
const WORKFLOW_EXTENSIONS: &[&str] = &["ts", "js"];

/// Workflow lookup directories, in precedence order: the coco namespace
/// (`<config-dir>/workflows`) before the `.claude/workflows` fallback. Built
/// from the shared config-dir constant so the namespace never drifts.
fn workflow_dirs(cwd: &Path) -> [PathBuf; 2] {
    [
        cwd.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join(WORKFLOW_SUBDIR),
        cwd.join(CLAUDE_CONFIG_DIR).join(WORKFLOW_SUBDIR),
    ]
}

/// Human-readable workflow lookup directories for tool prompts/descriptions,
/// in precedence order. Derived from the same config-dir constant as
/// [`workflow_dirs`] so model-facing text never hardcodes the namespace or
/// drifts from the actual lookup paths.
pub fn workflow_dirs_hint() -> String {
    format!(
        "{}/{} or {}/{}",
        coco_utils_common::COCO_CONFIG_DIR_NAME,
        WORKFLOW_SUBDIR,
        CLAUDE_CONFIG_DIR,
        WORKFLOW_SUBDIR
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowSourceInput {
    pub script_path: Option<PathBuf>,
    pub name: Option<String>,
    pub script: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowSourceKind {
    ScriptPath(PathBuf),
    Name(String),
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSourceSpec {
    pub kind: WorkflowSourceKind,
    pub source: String,
    pub source_path: Option<PathBuf>,
}

/// Where a workflow definition came from.
///
/// Also encodes precedence: a local file **shadows** a bundled workflow with
/// the same `meta.name` (mirroring the reference's built-in < plugin < user
/// ordering — your machine, your workflow). The picker shows the origin so a
/// shadowed built-in is visible as such rather than silently gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOrigin {
    /// Compiled into the binary ([`crate::bundled`]).
    Bundled,
    /// Loaded from a file under one of the [`workflow_dirs`].
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRegistryEntry {
    pub meta: crate::WorkflowMeta,
    pub origin: WorkflowOrigin,
}

pub fn resolve_workflow_source(input: WorkflowSourceInput) -> Result<WorkflowSourceSpec> {
    let cwd = input.cwd.unwrap_or_else(|| PathBuf::from("."));
    if let Some(script_path) = input.script_path {
        // Reject UNC on the RAW input before the cwd join: a backslash-UNC
        // (`\\server\share`) is not absolute on Linux, so joining it to cwd
        // would hide the leading `\\` from the post-join guard.
        reject_unc(&script_path)?;
        let path = resolve_script_path(&cwd, script_path);
        reject_unc(&path)?;
        if let Some(script) = input.script {
            ensure_size(script.len())?;
            return Ok(WorkflowSourceSpec {
                kind: WorkflowSourceKind::ScriptPath(path.clone()),
                source: script,
                source_path: Some(path),
            });
        }
        return source_from_path(path, WorkflowSourceKind::ScriptPath);
    }

    if let Some(name) = input.name.filter(|s| !s.trim().is_empty()) {
        let (origin, found_source) = resolve_named_workflow(&cwd, &name).ok_or_else(|| {
            NamedWorkflowNotFoundSnafu {
                name: name.clone(),
                available: available_workflows_message(&cwd),
            }
            .build()
        })?;
        // Inline `script` overrides the registry body, but provenance (path) is
        // kept (TS: `input.script ?? found.script`).
        let source = match input.script.filter(|s| !s.is_empty()) {
            Some(script) => {
                ensure_size(script.len())?;
                script
            }
            None => found_source,
        };
        return Ok(WorkflowSourceSpec {
            kind: WorkflowSourceKind::Name(name),
            source,
            // A bundled workflow has no on-disk provenance. `None` here is what
            // makes the launcher persist the resolved source into the run's own
            // directory instead of pointing resume at a path that never existed.
            source_path: match origin {
                WorkflowOrigin::Local(path) => Some(path),
                WorkflowOrigin::Bundled => None,
            },
        });
    }

    if let Some(script) = input.script {
        ensure_size(script.len())?;
        return Ok(WorkflowSourceSpec {
            kind: WorkflowSourceKind::Inline,
            source: script,
            source_path: None,
        });
    }

    MissingSourceSnafu.fail()
}

/// Every workflow reachable by name, de-duplicated with local files taking
/// precedence over bundled ones (and, among local files, the first lookup dir
/// winning). Bundled entries come last so the list reads local-first.
pub fn list_workflows(cwd: Option<PathBuf>) -> Vec<WorkflowRegistryEntry> {
    let cwd = cwd.unwrap_or_else(|| PathBuf::from("."));
    let local = scan_workflow_registry(&cwd)
        .into_iter()
        .map(|(path, _, meta)| WorkflowRegistryEntry {
            meta,
            origin: WorkflowOrigin::Local(path),
        });
    let bundled =
        crate::bundled::bundled_workflows()
            .iter()
            .map(|workflow| WorkflowRegistryEntry {
                meta: workflow.meta.clone(),
                origin: WorkflowOrigin::Bundled,
            });
    let mut out: Vec<WorkflowRegistryEntry> = Vec::new();
    for entry in local.chain(bundled) {
        if out.iter().any(|seen| seen.meta.name == entry.meta.name) {
            continue;
        }
        out.push(entry);
    }
    out
}

fn resolve_script_path(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn source_from_path<F>(path: PathBuf, kind: F) -> Result<WorkflowSourceSpec>
where
    F: FnOnce(PathBuf) -> WorkflowSourceKind,
{
    reject_unc(&path)?;
    let bytes = read_capped(&path).map_err(|source| {
        ReadSourceSnafu {
            path: path.display().to_string(),
            message: source.to_string(),
        }
        .build()
    })?;
    ensure_size(bytes.len())?;
    let source = String::from_utf8(bytes).map_err(|_| InvalidUtf8Snafu.build())?;
    Ok(WorkflowSourceSpec {
        kind: kind(path.clone()),
        source,
        source_path: Some(path),
    })
}

fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(MAX_WORKFLOW_SOURCE_BYTES.min(8192));
    let limit = (MAX_WORKFLOW_SOURCE_BYTES + 1) as u64;
    file.by_ref().take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Resolve a named workflow to `(origin, source)` by matching the parsed
/// `meta.name` of each on-disk script —
/// over the `getAllWorkflows` registry, NOT the filename stem (a saved workflow
/// `My Build` is slugified to `my-build.js` yet invoked by its `meta.name`).
/// `.cocode/workflows` is searched before `.claude/workflows`; within a dir,
/// files are visited in sorted order for determinism. Because names are matched
/// against parsed metadata rather than used to build a path, name-based path
/// traversal is structurally impossible.
///
/// Bundled workflows are the **last** resort, so a local file of the same name
/// shadows a built-in outright rather than merging with it.
fn resolve_named_workflow(cwd: &Path, name: &str) -> Option<(WorkflowOrigin, String)> {
    let local = scan_workflow_registry(cwd)
        .into_iter()
        .find(|(_, _, meta)| meta.name == name)
        .map(|(path, source, _)| (WorkflowOrigin::Local(path), source));
    local.or_else(|| {
        crate::bundled::bundled_workflow(name)
            .map(|workflow| (WorkflowOrigin::Bundled, workflow.script.to_string()))
    })
}

/// The available workflow names (parsed `meta.name`), de-duplicated and sorted,
/// for the not-found error
/// with a `(none)` sentinel when empty.
fn available_workflows_message(cwd: &Path) -> String {
    let mut names: Vec<String> = scan_workflow_registry(cwd)
        .into_iter()
        .map(|(_, _, meta)| meta.name)
        .chain(
            crate::bundled::bundled_workflows()
                .iter()
                .map(|workflow| workflow.meta.name.clone()),
        )
        .collect();
    names.sort();
    names.dedup();
    let listed = if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    };
    format!(". Available: {listed}")
}

/// Scan the workflow lookup directories, returning `(path, source, meta)` for
/// every readable, in-size, parseable script. Files that don't read, exceed the
/// size cap, aren't UTF-8, or whose `meta` doesn't parse are silently skipped.
/// The determinism check is intentionally NOT run here — like TS
/// `parseWorkflowMeta`, registry indexing is independent of `isNonDeterministic`.
fn scan_workflow_registry(cwd: &Path) -> Vec<(PathBuf, String, crate::WorkflowMeta)> {
    let mut found = Vec::new();
    for dir in workflow_dirs(cwd) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| WORKFLOW_EXTENSIONS.contains(&ext))
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(bytes) = read_capped(&path) else {
                continue;
            };
            if bytes.len() > MAX_WORKFLOW_SOURCE_BYTES {
                continue;
            }
            let Ok(source) = String::from_utf8(bytes) else {
                continue;
            };
            let Ok(script) = crate::parse_workflow_script(&source, false) else {
                continue;
            };
            found.push((path, source, script.meta));
        }
    }
    found
}

fn reject_unc(path: &Path) -> Result<()> {
    let display = path.display().to_string();
    if display.starts_with("\\\\") || display.starts_with("//") {
        return UncPathSnafu { path: display }.fail();
    }
    Ok(())
}

fn ensure_size(actual: usize) -> Result<()> {
    if actual > MAX_WORKFLOW_SOURCE_BYTES {
        return SourceTooLargeSnafu {
            limit: MAX_WORKFLOW_SOURCE_BYTES,
            actual,
        }
        .fail();
    }
    Ok(())
}
