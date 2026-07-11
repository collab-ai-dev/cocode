//! Tool-input → write-target extraction for fenced `CanUseToolHandle`
//! policies.
//!
//! A write fence decides per path ("is this under my root? does the filename
//! pass policy?"), but *finding* the affected paths in a tool input is
//! policy-free mechanics that must not drift between fences: the write-tool
//! input keys (`file_path` / `notebook_path` / `path`), relative-path
//! resolution against the call cwd, and the `apply_patch` hunk expansion.
//! Memory's fence and the skill-review fence both go through these helpers
//! and apply only their own per-path predicate on top.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The write target of an `Edit` / `Write` / `NotebookEdit`-shaped input:
/// the first of `file_path` / `notebook_path` / `path`, resolved against
/// `cwd` when relative. `None` when no path key is present — callers treat
/// that as deny (fail closed).
pub fn input_write_target(input: &Value, cwd: &Path) -> Option<PathBuf> {
    let path = input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let candidate = Path::new(path);
    Some(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    })
}

/// Every path an `apply_patch` input touches, resolved against `cwd`.
/// `None` when the patch is missing/unparseable or touches nothing —
/// callers treat that as deny (fail closed).
pub fn apply_patch_write_targets(input: &Value, cwd: &Path) -> Option<Vec<PathBuf>> {
    let patch = input.get("patch").and_then(Value::as_str)?;
    let cwd = coco_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd).ok()?;
    let parsed = coco_apply_patch::parse_patch(patch).ok()?;
    let effects = coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd);
    if effects.permission_paths.is_empty() {
        return None;
    }
    Some(effects.permission_paths)
}
