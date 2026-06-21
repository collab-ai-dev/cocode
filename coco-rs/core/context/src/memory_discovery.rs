//! Eager memory-file discovery — root→CWD walk loaded once at session
//! start.
//!
//! Per-file lazy traversal lives in [`crate::nested_memory`] and is
//! driven by file-read triggers, not this module.
//!
//! **Naming**: coco-rs supports both
//! `CLAUDE.md` and `AGENTS.md` (Codex / Cursor convention) at every
//! eager-load position, matched case-insensitively via
//! [`crate::memory_filenames::find_memory_files`]. The struct is named
//! `MemoryFile` to reflect this — `ClaudeMdFile` is no longer used.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::memory_filenames::MEMORY_FILE_CANDIDATES;
use crate::memory_filenames::MEMORY_LOCAL_FILE_CANDIDATES;
use crate::memory_filenames::find_memory_files;
use crate::memory_imports::expand_imports;

/// A discovered memory file (`CLAUDE.md`, `AGENTS.md`, or local variant).
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub content: String,
    pub source: MemoryFileSource,
}

/// Where a memory file was found in the eager load.
///
/// Per-file lazy traversal (driven by file-read triggers) emits
/// `Project` for each loaded file regardless of relative depth — the
/// `path` field carries the precise location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFileSource {
    /// `/etc/coco/{CLAUDE,AGENTS}.md` + `/etc/coco/rules` — policy-level
    /// memory installed by an admin/MDM. Loaded first, always.
    Managed,
    /// `~/.coco/CLAUDE.md` (or AGENTS.md) — user-global.
    UserGlobal,
    /// `<dir>/.coco/CLAUDE.md` — project config dir.
    ProjectConfig,
    /// `<dir>/CLAUDE.md` or `<dir>/AGENTS.md` — root-level project file.
    Project,
    /// `<dir>/CLAUDE.local.md` or `<dir>/AGENTS.local.md` — gitignored.
    Local,
}

/// Discover all memory files for the given working directory.
///
/// Walk order:
/// 0. Managed `/etc/coco/{CLAUDE,AGENTS}.md` + unconditional `/etc/coco/rules`.
/// 1. User-global `~/.coco/{CLAUDE,AGENTS}.md` + unconditional `~/.coco/rules`.
/// 2. From filesystem root walking down to `cwd` inclusive, in each dir:
///    - `<dir>/.coco/CLAUDE.md` (project config dir; AGENTS.md not added here)
///    - `<dir>/{CLAUDE,AGENTS}.md` (case-insensitive)
///    - `<dir>/.coco/rules/*.md` **unconditional** rules (no `paths:`);
///      conditional rules stay in the lazy [`crate::nested_memory`] pass.
///    - `<dir>/{CLAUDE,AGENTS}.local.md` (case-insensitive)
///
/// `@import` expansion strips HTML comments and gates external (outside-cwd)
/// includes per tier (only user-global memory may include external files).
///
/// Files closer to `cwd` are loaded last → highest model-attention
/// priority ("Files are loaded in reverse order of priority"). Duplicates
/// resolved via canonicalized-path dedup (e.g.
/// when CWD == filesystem root or when symlinks loop back).
///
/// Per-file lazy traversal — adding `<between-cwd-and-file>/CLAUDE.md`
/// and conditional `.coco/rules/*.md` matches — happens in
/// [`crate::nested_memory`] driven by [`coco_tool_runtime`] file-read
/// triggers, not this function.
pub fn discover_memory_files(cwd: &Path) -> Vec<MemoryFile> {
    let mut files: Vec<MemoryFile> = Vec::new();
    // Shared `processed` set for the whole eager pass so:
    //   1. canonical-path dedup across positions (managed, user-global,
    //      project, .coco/, rules, local) without rescanning `files`.
    //   2. an `@import` chain that resolves into another would-be-loaded
    //      file is not double-loaded.
    let mut processed: HashSet<PathBuf> = HashSet::new();

    // 0. Managed (policy) `/etc/coco/{CLAUDE,AGENTS}.md` + unconditional
    //    `/etc/coco/rules`. Loaded first. Lowest model-attention, always applied.
    let managed_dir = managed_memory_dir();
    for path in find_memory_files(&managed_dir, MEMORY_FILE_CANDIDATES) {
        try_push(
            &path,
            MemoryFileSource::Managed,
            &mut files,
            &mut processed,
            cwd,
        );
    }
    push_unconditional_rules(
        &managed_dir.join("rules"),
        MemoryFileSource::Managed,
        &mut files,
        &mut processed,
        cwd,
    );

    // 1. User-global `~/.coco/{CLAUDE,AGENTS}.md` + unconditional `~/.coco/rules`.
    if let Some(home) = dirs_home() {
        let coco_dir = home.join(".coco");
        for path in find_memory_files(&coco_dir, MEMORY_FILE_CANDIDATES) {
            try_push(
                &path,
                MemoryFileSource::UserGlobal,
                &mut files,
                &mut processed,
                cwd,
            );
        }
        push_unconditional_rules(
            &coco_dir.join("rules"),
            MemoryFileSource::UserGlobal,
            &mut files,
            &mut processed,
            cwd,
        );
    }

    // 2. Walk root→cwd inclusive. Build dirs from cwd up, then reverse
    //    so loading proceeds root→cwd.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        dirs.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    dirs.reverse();

    // When `cwd` is a git worktree nested inside its main repo (coco's agent
    // worktrees live at `<main>/.coco/worktrees/<slug>`), the root→cwd walk
    // passes through BOTH the main repo root and the worktree root. git checks
    // the branch's tracked memory (CLAUDE.md, `.coco/rules/*`, …) out into
    // the worktree, so the same content sits at two distinct paths and would
    // load twice. `nested` lets us skip the main repo's checked-in copy in the
    // dirs above the worktree. `None` for regular repos / non-repos → no skip.
    let nested = nested_worktree_roots(cwd);

    for dir in &dirs {
        // Skip the main repo's checked-in (Project / ProjectConfig /
        // unconditional-rules) files in dirs inside the main repo but above
        // the worktree — the worktree has its own checkout. `CLAUDE.local.md`
        // is gitignored (only in the main repo, never duplicated) so it stays
        // loaded below the guard.
        let skip_project = nested
            .as_ref()
            .is_some_and(|roots| dir_in_skip_zone(dir, roots));

        if !skip_project {
            // .coco/CLAUDE.md (project config dir; we don't extend with
            // AGENTS.md here since `.coco/` is the config dir convention,
            // not a memory dir).
            let dot_coco = dir.join(".coco").join("CLAUDE.md");
            try_push(
                &dot_coco,
                MemoryFileSource::ProjectConfig,
                &mut files,
                &mut processed,
                cwd,
            );

            // <dir>/{CLAUDE,AGENTS}.md (case-insensitive)
            for path in find_memory_files(dir, MEMORY_FILE_CANDIDATES) {
                try_push(
                    &path,
                    MemoryFileSource::Project,
                    &mut files,
                    &mut processed,
                    cwd,
                );
            }

            // <dir>/.coco/rules/*.md unconditional rules (no `paths:`
            // frontmatter) — always-on project guidance. Conditional
            // (`paths:`) rules stay in the lazy per-file traversal.
            push_unconditional_rules(
                &dir.join(".coco").join("rules"),
                MemoryFileSource::Project,
                &mut files,
                &mut processed,
                cwd,
            );
        }

        // <dir>/{CLAUDE,AGENTS}.local.md (case-insensitive). Loaded even when
        // skip_project: gitignored, so it only exists in the main repo.
        for path in find_memory_files(dir, MEMORY_LOCAL_FILE_CANDIDATES) {
            try_push(
                &path,
                MemoryFileSource::Local,
                &mut files,
                &mut processed,
                cwd,
            );
        }
    }

    files
}

/// Resolve `(worktree_root, canonical_root)` when `cwd` sits inside a git
/// worktree nested under its main repository, else `None`.
///
/// - worktree root = `git rev-parse --show-toplevel`, via
///   [`crate::git_utils::get_git_root`].
/// - canonical root = [`coco_git::find_canonical_git_root`], via
///   `git rev-parse --git-common-dir`.
///
/// Both are canonicalized before comparison so a symlink-unresolved
/// `--show-toplevel` (e.g. macOS `/tmp` → `/private/tmp`) can't defeat the
/// `!=` / inside checks. "Nested" = the two differ AND the worktree root sits
/// inside the canonical root.
///
/// Security: delegating canonical-root resolution to `git rev-parse` means we
/// inherit git's own gitdir/commondir validation. The hand-rolled `.git` /
/// `commondir` parsing that must be security-checked (to stop a malicious repo
/// redirecting `commondir` at a trusted path) doesn't exist here, and this skip
/// path executes nothing — a mis-resolution is a memory-content quirk, not a
/// trust/hook-execution bypass (that surface is settings.json loading, a
/// separate path).
///
/// Not memoized; discovery runs at session start + per subagent spawn, so the
/// two `git` calls are off the hot path.
pub(crate) fn nested_worktree_roots(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let worktree_root = canon_or_self(Path::new(&crate::git_utils::get_git_root(cwd)?));
    let canonical_root = canon_or_self(&coco_git::find_canonical_git_root(cwd)?);
    (worktree_root != canonical_root && worktree_root.starts_with(&canonical_root))
        .then_some((worktree_root, canonical_root))
}

/// True iff `dir` is in the skip zone of a nested worktree: inside the main
/// repo (`canonical_root`) but above the worktree (`worktree_root`). Checked-in
/// memory there is also checked out into the worktree, so loading it would
/// duplicate the worktree's own copy. `dir` is canonicalized before comparison;
/// the roots are already canonical from [`nested_worktree_roots`].
pub(crate) fn dir_in_skip_zone(dir: &Path, roots: &(PathBuf, PathBuf)) -> bool {
    let (worktree_root, canonical_root) = roots;
    let dir = canon_or_self(dir);
    dir.starts_with(canonical_root) && !dir.starts_with(worktree_root)
}

/// Best-effort canonicalize; falls back to the input for non-existent paths.
fn canon_or_self(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Policy-level managed memory directory. Mirrors coco's existing managed
/// convention (`/etc/coco/rules`, [`crate::nested_memory`]); missing on
/// non-Linux hosts, in which case no managed memory loads.
fn managed_memory_dir() -> PathBuf {
    PathBuf::from("/etc/coco")
}

/// Whether `@import` of files OUTSIDE the project cwd is permitted for this
/// memory tier. Only user-global memory may pull in external files; every
/// other tier uses the default-off project flag.
pub(crate) fn allows_external_imports(source: MemoryFileSource) -> bool {
    matches!(source, MemoryFileSource::UserGlobal)
}

fn try_push(
    path: &Path,
    source: MemoryFileSource,
    files: &mut Vec<MemoryFile>,
    processed: &mut HashSet<PathBuf>,
    cwd: &Path,
) {
    if !path.exists() {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    // expand_imports owns the canonical-path dedup against `processed`,
    // the `@import` recursion's cycle break, HTML-comment stripping, and
    // the external-import security gate. Returns the parent first followed
    // by transitively-included files.
    for (p, c) in expand_imports(
        path,
        &content,
        processed,
        0,
        cwd,
        allows_external_imports(source),
    ) {
        files.push(MemoryFile {
            path: p,
            content: c,
            source,
        });
    }
}

/// Eager-load unconditional `.coco/rules/*.md` (no `paths:` frontmatter)
/// from `rules_dir` as `source`-tagged memory, sharing the `processed` dedup
/// set so a rule that's also `@import`ed is not loaded twice.
fn push_unconditional_rules(
    rules_dir: &Path,
    source: MemoryFileSource,
    files: &mut Vec<MemoryFile>,
    processed: &mut HashSet<PathBuf>,
    cwd: &Path,
) {
    for rule in crate::memory_rules::collect_rule_files(rules_dir, /*conditional*/ false) {
        for (p, c) in expand_imports(
            &rule.path,
            &rule.content,
            processed,
            0,
            cwd,
            allows_external_imports(source),
        ) {
            files.push(MemoryFile {
                path: p,
                content: c,
                source,
            });
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
#[path = "memory_discovery.test.rs"]
mod tests;
