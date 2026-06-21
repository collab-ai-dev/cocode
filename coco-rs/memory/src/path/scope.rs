//! Memory file classification — scope (personal/team) + auto-managed
//! detection used by the secret-guard, permission carve-outs, and the
//! file-history skip list.

use std::path::Path;

use super::validate::is_within_memory_dir;

/// Scope of a memory file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Personal,
    Team,
}

/// Type of a session-related file (transcript vs session memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileType {
    Memory,
    Transcript,
    Other,
}

/// Predicate: is `path` under `memory_dir.personal`?
pub fn is_auto_mem_file(path: &Path, memory_dir: &Path) -> bool {
    is_within_memory_dir(path, memory_dir)
}

/// Anything we manage automatically: personal memory, team memory,
/// session memory file. Used by secret-guard / permission carve-outs.
pub fn is_auto_managed_memory_file(path: &Path, memory_dir: &Path) -> bool {
    is_auto_mem_file(path, memory_dir) || path.to_string_lossy().contains("session_memory")
}

/// Personal vs team — checked by directory layout. Team is a strict
/// subdir of personal so the team check goes first.
pub fn memory_scope_for_path(path: &Path, memory_dir: &Path) -> MemoryScope {
    let team = memory_dir.join("team");
    if is_within_memory_dir(path, &team) {
        MemoryScope::Team
    } else {
        MemoryScope::Personal
    }
}

/// Permission carve-out: auto-memory paths skip the dangerous-dirs
/// prompt so the agent can save without an extra confirmation. Disabled
/// when the operator installed a custom path override.
pub fn should_bypass_dangerous_dirs(
    path: &Path,
    memory_dir: &Path,
    has_path_override: bool,
) -> bool {
    if has_path_override {
        return false;
    }
    is_auto_mem_file(path, memory_dir)
}

/// Classification of a single tool-write path against the memory
/// taxonomy. Used by `MemoryRuntime::finalize_turn`'s Gap 4 pass to
/// decide whether a tool's `Edit` / `Write` / `NotebookEdit` should
/// surface a `ManualEdit` notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteClassification {
    /// Path is under `<memdir>/team/...` — the model edited a team
    /// memory file. Highest specificity check (team is a strict
    /// subdir of personal, so team is matched first).
    TeamMem,
    /// Path is under `<memdir>/...` but NOT inside the team subtree.
    /// Includes top-level topic files and any subdir except `team/`.
    AutoMem,
    /// Path matches the session-memory file (per-session `summary.md`).
    /// Distinct from auto-mem because session-memory is a fundamentally
    /// different artifact with its own lifecycle.
    SessionMem,
    /// Path is one of the curated `CLAUDE.md` / `AGENTS.md` / `.coco/rules/`
    /// files in user, project, or local scope. Captured as a single
    /// variant — the dialog handles the finer scope distinction.
    Claudemd,
    /// Path is not a memory file.
    Unrelated,
}

/// Classify `path` against the memory taxonomy. Empty / unparseable
/// paths return `Unrelated`. Ordering matters: TeamMem before AutoMem
/// (team is a strict subdir of memdir); SessionMem matched by
/// filename substring; Claudemd is the fallback for any known
/// curated-file basename.
pub fn classify_written_path(
    path: &Path,
    memory_dir: &Path,
    session_memory_file: Option<&Path>,
) -> WriteClassification {
    // Session memory file — exact path match. Cheaper than the
    // substring check and avoids false positives on user files
    // that happen to be named `session_memory.md`.
    if let Some(sm) = session_memory_file
        && path == sm
    {
        return WriteClassification::SessionMem;
    }
    // Team subtree first (it's a subdir of memory_dir).
    let team_dir = memory_dir.join("team");
    if is_within_memory_dir(path, &team_dir) {
        return WriteClassification::TeamMem;
    }
    if is_within_memory_dir(path, memory_dir) {
        return WriteClassification::AutoMem;
    }
    // Curated CLAUDE.md / AGENTS.md / .coco/rules/*.md by basename.
    // Cheaper than a recursive containment check — these files have a
    // stable naming convention.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        match name {
            "CLAUDE.md" | "AGENTS.md" | "CLAUDE.local.md" => {
                return WriteClassification::Claudemd;
            }
            _ => {}
        }
        // `.coco/rules/<anything>.md`
        if path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == ".coco")
            && path
                .components()
                .any(|c| c.as_os_str().to_string_lossy() == "rules")
            && name.ends_with(".md")
        {
            return WriteClassification::Claudemd;
        }
    }
    WriteClassification::Unrelated
}

#[cfg(test)]
#[path = "scope.test.rs"]
mod tests;
