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

pub fn resolve_workflow_source(input: WorkflowSourceInput) -> Result<WorkflowSourceSpec> {
    let cwd = input
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
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
        let (path, found_source) = resolve_named_workflow(&cwd, &name).ok_or_else(|| {
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
            source_path: Some(path),
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

/// Resolve a named workflow to `(path, source)` by matching the parsed
/// `meta.name` of each on-disk script — mirroring TS `resolveNamedWorkflow`
/// over the `getAllWorkflows` registry, NOT the filename stem (a saved workflow
/// `My Build` is slugified to `my-build.js` yet invoked by its `meta.name`).
/// `.cocode/workflows` is searched before `.claude/workflows`; within a dir,
/// files are visited in sorted order for determinism. Because names are matched
/// against parsed metadata rather than used to build a path, name-based path
/// traversal is structurally impossible.
fn resolve_named_workflow(cwd: &Path, name: &str) -> Option<(PathBuf, String)> {
    scan_workflow_registry(cwd)
        .into_iter()
        .find(|(_, _, meta)| meta.name == name)
        .map(|(path, source, _)| (path, source))
}

/// The available workflow names (parsed `meta.name`), de-duplicated and sorted,
/// for the not-found error — matches TS `getAllWorkflows(cwd).map(w => w.name)`
/// with a `(none)` sentinel when empty.
fn available_workflows_message(cwd: &Path) -> String {
    let mut names: Vec<String> = scan_workflow_registry(cwd)
        .into_iter()
        .map(|(_, _, meta)| meta.name)
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
