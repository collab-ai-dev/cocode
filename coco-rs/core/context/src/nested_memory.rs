//! Per-file lazy memory traversal — fired by [`coco_tool_runtime`] file-read
//! triggers, not by static prompt-build.
//!
//! The eager `discover_memory_files` (in `claudemd.rs`) loads the
//! `root → CWD` slice once at session start. This module fills in the
//! slice **strictly between CWD (exclusive) and the read file's parent
//! (inclusive)** — files Claude needs after the user reads them, not
//! eagerly speculatively.
//!
//! ## Phases
//!
//! For trigger file `X`:
//! 1. **Phase 1** — managed/user **conditional** rules matching `X`'s path
//!    glob. Wired in [Phase 4 of the optimization plan]; this module
//!    leaves the seam (`Vec::new()` placeholder).
//! 2. **Phase 2** — split dirs via [`directories_to_process`].
//! 3. **Phase 3** — for each `nested_dir`, load
//!    `{CLAUDE,AGENTS}.md`, `.coco/CLAUDE.md`, `{CLAUDE,AGENTS}.local.md`,
//!    and (Phase 4) all matching rules.
//! 4. **Phase 4** — for each `cwd_level_dir`, load only matching
//!    conditional rules.
//!
//! ## Filename-matching divergence
//!
//! The original only matches `CLAUDE.md` and `CLAUDE.local.md` literally.
//! coco-rs also accepts `AGENTS.md` / `AGENTS.local.md` and is case-insensitive
//! at every position — see [`crate::memory_filenames`].

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::memory_discovery::MemoryFileSource;
use crate::memory_filenames::MEMORY_FILE_CANDIDATES;
use crate::memory_filenames::MEMORY_LOCAL_FILE_CANDIDATES;
use crate::memory_filenames::find_memory_files;
use crate::memory_imports::expand_imports;
use crate::memory_rules::collect_rule_files;
use crate::memory_rules::filter_rules_matching;
use crate::memory_rules::rule_to_entry;

/// One memory file loaded by per-file traversal.
///
/// Caller-side conversion to `coco_system_reminder::NestedMemoryInfo`
/// happens in `app/query` so this crate stays free of a system-reminder
/// dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedMemoryEntry {
    pub path: PathBuf,
    /// Content surfaced to the model after memory transforms.
    pub content: String,
    /// Raw file content from disk before transforms such as HTML comment
    /// stripping, frontmatter stripping, or import expansion.
    pub raw_content: String,
    pub source: MemoryFileSource,
}

/// Split the filesystem into the two zones the per-file traversal walks:
///
/// - `nested_dirs`: directories strictly between `cwd` (exclusive) and
///   the file's parent (inclusive), filtered to `startsWith(cwd)`.
///   Order: `cwd`-side → file-side. Each dir gets a full memory load
///   (CLAUDE.md, .coco/CLAUDE.md, local, rules — both unconditional
///   and matching conditional).
/// - `cwd_level_dirs`: filesystem root → `cwd` inclusive. Order:
///   root → `cwd`. Each dir contributes only conditional rules
///   matching the trigger file (unconditional content for these dirs
///   was loaded eagerly at session start).
///
/// **CWD itself is in `cwd_level_dirs`, NOT in `nested_dirs`** —
/// preserving this invariant is what prevents the eager and lazy
/// phases from double-loading `cwd/CLAUDE.md`.
///
/// Files outside `cwd` (e.g. `/etc/foo.conf` with cwd `/proj`) yield
/// an empty `nested_dirs`; only Phase 1 + Phase 4 fire.
pub fn directories_to_process(file: &Path, cwd: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    // `cwd` may not exist; canonicalize is best-effort. Without it,
    // path-equality checks misbehave under `..` segments.
    let cwd_norm = canon_or_self(cwd);
    let target_parent = file
        .parent()
        .map(canon_or_self)
        .unwrap_or_else(|| cwd_norm.clone());

    // nested_dirs: walk up from target_parent to cwd, collecting dirs
    // that are descendants of cwd (exclusive of cwd itself).
    let mut nested_dirs: Vec<PathBuf> = Vec::new();
    let mut current = target_parent;
    loop {
        if current == cwd_norm {
            break;
        }
        if !current.starts_with(&cwd_norm) {
            // Walked past CWD or never under it (file outside CWD).
            break;
        }
        nested_dirs.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    nested_dirs.reverse(); // cwd-side first → file-side last

    // cwd_level_dirs: root → cwd inclusive. Build cwd-up, then reverse.
    let mut cwd_level_dirs: Vec<PathBuf> = Vec::new();
    let mut current = cwd_norm;
    loop {
        cwd_level_dirs.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    cwd_level_dirs.reverse();

    (nested_dirs, cwd_level_dirs)
}

/// Per-file traversal entry point. Loads the memory files surfaced by
/// reading `file` at the current `cwd`. `loaded` is the session-level
/// dedup set — already-loaded paths are skipped and newly-loaded paths
/// are inserted before the function returns.
///
/// Phases 1 and 4 (managed/user + cwd-level conditional rules) are
/// stubbed in this revision — the surface accepts their future output
/// without changing. Phase 3 (nested-dir base + local files) is fully
/// wired so trigger reads inside a subtree of CWD pick up the
/// in-between memory files immediately.
pub fn traverse_for_file(
    file: &Path,
    cwd: &Path,
    loaded: &mut HashSet<PathBuf>,
) -> Vec<LoadedMemoryEntry> {
    let mut out: Vec<LoadedMemoryEntry> = Vec::new();

    // Phase 1: managed/user conditional rules — wired in Phase 4 of the
    // optimization plan. Stub now keeps the call shape stable.
    out.extend(phase1_managed_user_conditional_rules(file));

    let (nested_dirs, cwd_level_dirs) = directories_to_process(file, cwd);

    // Phase 3: per-nested-dir CLAUDE.md / AGENTS.md / .coco/CLAUDE.md /
    // local + .coco/rules/**/*.md (unconditional + matching conditional).
    for dir in &nested_dirs {
        load_nested_dir(dir, file, cwd, &mut out, loaded);
    }

    // Phase 4: cwd-level conditional rules. For dirs from filesystem
    // root → CWD inclusive, only conditional `.coco/rules/**/*.md`
    // matching the trigger file are loaded — unconditional rules in
    // those dirs were already loaded eagerly at session start.
    //
    // In a nested worktree, skip the main repo's dirs above the worktree:
    // a conditional rule there is also checked out into the worktree (loaded
    // via Phase 3 / cwd's own dir), so loading the main-repo copy too would
    // double the same guidance at a different path. Mirrors the eager skip
    // (claudemd.ts:881-884). Phase 3 `nested_dirs` are descendants of cwd
    // (inside the worktree) so they never hit the skip zone.
    let nested = crate::memory_discovery::nested_worktree_roots(cwd);
    for dir in &cwd_level_dirs {
        if nested
            .as_ref()
            .is_some_and(|roots| crate::memory_discovery::dir_in_skip_zone(dir, roots))
        {
            continue;
        }
        load_cwd_level_conditional_rules(dir, file, cwd, &mut out, loaded);
    }

    out
}

fn load_nested_dir(
    dir: &Path,
    file: &Path,
    cwd: &Path,
    out: &mut Vec<LoadedMemoryEntry>,
    loaded: &mut HashSet<PathBuf>,
) {
    // <dir>/{CLAUDE,AGENTS}.md (case-insensitive). Project source.
    for path in find_memory_files(dir, MEMORY_FILE_CANDIDATES) {
        push_loaded(path, MemoryFileSource::Project, cwd, out, loaded);
    }

    // <dir>/.coco/CLAUDE.md (config-dir path).
    let dot_coco = dir.join(".coco").join("CLAUDE.md");
    if dot_coco.exists() {
        push_loaded(dot_coco, MemoryFileSource::ProjectConfig, cwd, out, loaded);
    }

    // <dir>/{CLAUDE,AGENTS}.local.md. Local source.
    for path in find_memory_files(dir, MEMORY_LOCAL_FILE_CANDIDATES) {
        push_loaded(path, MemoryFileSource::Local, cwd, out, loaded);
    }

    // <dir>/.coco/rules/**/*.md — both unconditional (descendants of CWD
    // weren't covered by the eager phase) and conditional matching the
    // trigger file.
    let rules_dir = dir.join(".coco").join("rules");
    if !rules_dir.exists() {
        return;
    }
    // Unconditional rules: load all files in this dir's rules tree
    // that don't have a `paths:` frontmatter.
    for rule in collect_rule_files(&rules_dir, false) {
        push_rule_entry(rule, MemoryFileSource::Project, cwd, out, loaded);
    }
    // Conditional rules: load files whose `paths:` glob matches the
    // trigger file. Project rules use `dirname(dirname(rules_dir))` as
    // the glob base.
    let project_base = dir.to_path_buf();
    let conditional = collect_rule_files(&rules_dir, true);
    let matched = filter_rules_matching(conditional, file, &project_base);
    for rule in matched {
        push_rule_entry(rule, MemoryFileSource::Project, cwd, out, loaded);
    }
}

fn push_rule_entry(
    rule: crate::memory_rules::RuleFile,
    source: MemoryFileSource,
    cwd: &Path,
    out: &mut Vec<LoadedMemoryEntry>,
    loaded: &mut HashSet<PathBuf>,
) {
    // Rule body is already frontmatter-stripped by `read_rule_file`.
    // Run @import expansion against `loaded` so includes from a rule
    // body load alongside it AND dedup against the session set.
    let entries = expand_imports(
        &rule.path,
        &rule.content,
        loaded,
        0,
        cwd,
        crate::memory_discovery::allows_external_imports(source),
    );
    let mut iter = entries.into_iter();
    if let Some((_p, _c)) = iter.next() {
        // Preserve the rule's typed shape for the first entry (this keeps
        // future extensions like `paths` available downstream).
        out.push(rule_to_entry(rule, source));
    }
    for (p, c) in iter {
        out.push(loaded_entry(p, c, source));
    }
}

fn push_loaded(
    path: PathBuf,
    source: MemoryFileSource,
    cwd: &Path,
    out: &mut Vec<LoadedMemoryEntry>,
    loaded: &mut HashSet<PathBuf>,
) {
    // Read first; on failure, leave `loaded` untouched so a transient
    // read error doesn't poison the session-level dedup set.
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    // expand_imports owns the canonical-path dedup against `loaded`
    // (passed as its `processed` set) AND the @import recursion's own
    // cycle break — single source of truth for both. Returns the
    // parent followed by each transitively-included file.
    let entries = expand_imports(
        &path,
        &content,
        loaded,
        0,
        cwd,
        crate::memory_discovery::allows_external_imports(source),
    );
    for (p, c) in entries {
        out.push(loaded_entry(p, c, source));
    }
}

fn loaded_entry(path: PathBuf, content: String, source: MemoryFileSource) -> LoadedMemoryEntry {
    let raw_content = std::fs::read_to_string(&path).unwrap_or_else(|_| content.clone());
    LoadedMemoryEntry {
        path,
        content,
        raw_content,
        source,
    }
}

/// Phase 1: load managed (`/etc/coco/rules/**/*.md`) and user
/// (`~/.coco/rules/**/*.md`) conditional rules whose `paths:` glob
/// matches `file`.
///
/// Glob base for managed/user rules is the original CWD.
fn phase1_managed_user_conditional_rules(file: &Path) -> Vec<LoadedMemoryEntry> {
    let mut out: Vec<LoadedMemoryEntry> = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_default();

    // Managed: /etc/coco/rules (best-effort; missing dirs are silent).
    let managed_dir = std::path::PathBuf::from("/etc/coco/rules");
    let managed_conditional = collect_rule_files(&managed_dir, true);
    for rule in filter_rules_matching(managed_conditional, file, &cwd) {
        out.push(rule_to_entry(rule, MemoryFileSource::Project));
    }

    // User: ~/.coco/rules.
    if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from) {
        let user_dir = home.join(".coco").join("rules");
        let user_conditional = collect_rule_files(&user_dir, true);
        for rule in filter_rules_matching(user_conditional, file, &cwd) {
            out.push(rule_to_entry(rule, MemoryFileSource::UserGlobal));
        }
    }

    out
}

fn load_cwd_level_conditional_rules(
    dir: &Path,
    file: &Path,
    cwd: &Path,
    out: &mut Vec<LoadedMemoryEntry>,
    loaded: &mut HashSet<PathBuf>,
) {
    let rules_dir = dir.join(".coco").join("rules");
    if !rules_dir.exists() {
        return;
    }
    let conditional = collect_rule_files(&rules_dir, true);
    let matched = filter_rules_matching(conditional, file, dir);
    for rule in matched {
        push_rule_entry(rule, MemoryFileSource::Project, cwd, out, loaded);
    }
}

fn canon_or_self(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
#[path = "nested_memory.test.rs"]
mod tests;
