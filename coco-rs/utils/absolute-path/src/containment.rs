//! Symlink-aware path containment — the shared fence primitive.
//!
//! A fenced background agent (memory extraction, skill review, …) may only
//! write inside a designated root directory, and untrusted `@import`
//! references may only read inside the project cwd. Deciding "is `candidate`
//! under `root`?" safely is subtle: a naive `starts_with` is defeated by `..`
//! traversal and by symlinks planted inside the root that resolve outside it.
//! This module is the single, security-critical implementation of that check;
//! every fenced subsystem must call it rather than re-deriving the logic.
//!
//! [`contains_symlink_aware`] is **fail-closed**: any ambiguity (ELOOP,
//! dangling symlink, EACCES, asymmetric realpath outcome) denies containment.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Lexically normalize a path: collapse `.` and `..` components without
/// touching the filesystem. Purely textual — collapsing `..` against a symlink
/// component is not kernel-faithful, so this must not be used to pre-normalize
/// a path before symlink resolution (see [`realpath_deepest_existing`], which
/// resolves symlinks on real prefixes first and only then applies `..` in the
/// non-existent tail).
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Resolve symlinks for the deepest existing ancestor of `path`, then re-apply
/// the non-existent tail with lexical `.`/`..` semantics.
///
/// If `path` exists, returns its canonical form. Otherwise it finds the
/// deepest existing prefix, canonicalizes it (resolving *its* symlinks), and
/// applies the remaining components on top: `..` pops, `.` is skipped, names
/// are pushed. The tail is by definition non-existent and therefore
/// symlink-free, so applying `..` as a pop is exact — and because symlinks are
/// only ever resolved on real prefixes, a `..` can never lexically cancel a
/// symlink (which would let `<root>/link/../escape` slip the containment
/// check). Applying `..` here rather than dropping it is load-bearing: a
/// verbatim re-append would let `<root>/a/../../x` deceptively `starts_with`
/// `<root>`.
///
/// Returns `None` when:
/// - no prefix is canonicalizable (effectively never — `/` always resolves), OR
/// - a prefix canonicalize hits a non-recoverable error (`PermissionDenied`,
///   ELOOP-shaped, …) — fail closed, OR
/// - `path` itself is a **dangling symlink** (probed via `symlink_metadata`
///   once `canonicalize` returns `NotFound`).
pub fn realpath_deepest_existing(path: &Path) -> Option<PathBuf> {
    match path.canonicalize() {
        Ok(real) => return Some(real),
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                // The leaf may not exist yet, or a tail component may be a
                // dangling symlink — probe the latter before rebuilding, so a
                // planted symlink whose target was deleted can't slip past.
                if let Ok(meta) = path.symlink_metadata()
                    && meta.file_type().is_symlink()
                {
                    return None;
                }
            }
            // ELOOP, EACCES, EIO, etc. — fail closed.
            _ => return None,
        },
    }
    // Peel components off the end until the remaining prefix canonicalizes,
    // then re-apply the peeled tail with `.`/`..` semantics.
    let comps: Vec<Component> = path.components().collect();
    for split in (1..comps.len()).rev() {
        let mut prefix = PathBuf::new();
        for comp in &comps[..split] {
            prefix.push(comp.as_os_str());
        }
        match prefix.canonicalize() {
            Ok(mut out) => {
                for comp in &comps[split..] {
                    match comp {
                        Component::ParentDir => {
                            out.pop();
                        }
                        Component::CurDir => {}
                        other => out.push(other.as_os_str()),
                    }
                }
                return Some(out);
            }
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {}
                _ => return None,
            },
        }
    }
    None
}

/// True when `candidate` is contained by `root` (equal to or a descendant of).
///
/// Symlink-aware and fail-closed. Each side is resolved by
/// [`realpath_deepest_existing`], which follows existing symlinks with kernel
/// semantics and applies `..` in any non-existent tail lexically — so a `..`
/// after a real symlink resolves against the symlink's *target* (never
/// cancelled against the link name), while a `..` in a purely hypothetical
/// path still collapses. The decision is then a component-wise `starts_with`
/// on the resolved forms.
///
/// Fail-closed matrix:
/// - both sides resolve → `candidate.starts_with(root)` on the resolved forms.
/// - neither resolves (both hypothetical — never on a normal filesystem, where
///   `/` always resolves) → lexical `starts_with`.
/// - asymmetric (one resolves, the other doesn't — e.g. a dangling symlink
///   planted inside `root`) → **`false`** (deny), with a warning.
pub fn contains_symlink_aware(root: &Path, candidate: &Path) -> bool {
    let real_root = realpath_deepest_existing(root);
    let real_candidate = realpath_deepest_existing(candidate);
    match (real_root.as_ref(), real_candidate.as_ref()) {
        (Some(rr), Some(rc)) => rc.starts_with(rr),
        (None, None) => lexical_normalize(candidate).starts_with(lexical_normalize(root)),
        _ => {
            tracing::warn!(
                target: "coco_utils_absolute_path::containment",
                root = %root.display(),
                candidate = %candidate.display(),
                root_resolved = real_root.is_some(),
                candidate_resolved = real_candidate.is_some(),
                "containment check: asymmetric realpath outcome, failing closed"
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "containment.test.rs"]
mod tests;
