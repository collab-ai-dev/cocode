//! `ProjectPaths` — single facade for every per-project filesystem
//! path layout coco-rs needs.
//!
//! TS layout mirrored (all paths rooted at `<memory_base>/projects/<slug>/`):
//!
//! | path | TS source | purpose |
//! |---|---|---|
//! | `<slug>/`                                           | `memdir/paths.ts:229` | project dir |
//! | `<slug>/<sid>.jsonl`                                | `sessionStorage.ts:202-225` | transcript |
//! | `<slug>/<sid>/`                                     | (artifact root)               | session dir |
//! | `<slug>/<sid>/subagents/agent-<id>.jsonl`           | `sessionStorage.ts:247-258` | bg agent transcript |
//! | `<slug>/<sid>/subagents/<subdir>/agent-<id>.jsonl`  | `sessionStorage.ts:247-258` | workflows/{runId} variant |
//! | `<slug>/<sid>/subagents/agent-<id>.meta.json`       | `sessionStorage.ts:260-262` | bg agent sidecar |
//! | `<slug>/<sid>/remote-agents/remote-agent-<tid>.meta.json` | `sessionStorage.ts:305-318` | CCR remote task sidecar |
//! | `<slug>/<sid>/tool-results/`                        | `toolResultStorage.ts:104-106` | persisted tool blobs |
//! | `<slug>/<sid>/session-memory/summary.md`            | `services/SessionMemory/sessionMemory.ts` | per-session notes |
//! | `<slug>/<sid>/usage.json`                            | (coco-rs) | per-session usage snapshot |
//! | `<slug>/memory/`                                    | `memdir/paths.ts:231` | personal auto-memory |
//! | `<slug>/memory/MEMORY.md`                           | `memdir/paths.ts:257-259` | personal index |
//! | `<slug>/memory/team/`                               | `memdir/teamMemPaths.ts:84-94` | team auto-memory root |
//! | `<slug>/memory/team/MEMORY.md`                      | `memdir/teamMemPaths.ts:90-94` | team index |
//! | `<slug>/memory/logs/YYYY/MM/YYYY-MM-DD.md`          | `memdir/paths.ts:246-251` | KAIROS daily log |
//! | `<slug>/memory/.consolidate-lock`                   | `services/autoDream/consolidationLock.ts:22` | auto-dream lock |
//!
//! Construction is cheap (one NFC pass and one linear sanitize) and
//! every accessor is an infallible `join`. Share via `Arc` when the
//! same project is referenced from multiple subsystems.

use std::path::{Path, PathBuf};

use crate::slug::ProjectSlug;

/// Per-project filesystem paths. Created once per (memory_base,
/// canonical_project_root) pair and reused for every subsequent
/// query.
#[derive(Debug, Clone)]
pub struct ProjectPaths {
    memory_base: PathBuf,
    slug: ProjectSlug,
}

impl ProjectPaths {
    /// Build the paths from the resolved memory base (typically
    /// `coco_config::config_home()`, overridable via
    /// `COCO_REMOTE_MEMORY_DIR`) and the project root.
    ///
    /// `project_root` should be the canonical git root via
    /// `coco_git::find_canonical_git_root`, falling back to the cwd
    /// itself when not in a git repo. Worktree resolution is the
    /// caller's responsibility — this function takes whatever path
    /// is provided and slugs it deterministically.
    pub fn new(memory_base: PathBuf, project_root: &Path) -> Self {
        Self {
            memory_base,
            slug: ProjectSlug::for_path(project_root),
        }
    }

    /// Build the paths directly from an already-computed slug. Used
    /// when the caller resolved the slug via a worktree-list scan
    /// (long-path prefix fallback) instead of slugging fresh.
    pub fn from_slug(memory_base: PathBuf, slug: ProjectSlug) -> Self {
        Self { memory_base, slug }
    }

    pub fn memory_base(&self) -> &Path {
        &self.memory_base
    }

    pub fn slug(&self) -> &ProjectSlug {
        &self.slug
    }

    // ---- top-level project paths --------------------------------

    /// `<memory_base>/projects/`
    pub fn projects_root(&self) -> PathBuf {
        self.memory_base.join("projects")
    }

    /// `<memory_base>/projects/<slug>/`
    pub fn project_dir(&self) -> PathBuf {
        self.projects_root().join(self.slug.as_str())
    }

    // ---- session paths ------------------------------------------

    pub fn transcript(&self, session_id: &str) -> PathBuf {
        self.project_dir().join(format!("{session_id}.jsonl"))
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.project_dir().join(session_id)
    }

    pub fn subagents_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("subagents")
    }

    pub fn agent_transcript(&self, session_id: &str, agent_id: &str) -> PathBuf {
        self.subagents_dir(session_id)
            .join(format!("agent-{agent_id}.jsonl"))
    }

    pub fn agent_transcript_in_subdir(
        &self,
        session_id: &str,
        subdir: &str,
        agent_id: &str,
    ) -> PathBuf {
        self.subagents_dir(session_id)
            .join(subdir)
            .join(format!("agent-{agent_id}.jsonl"))
    }

    pub fn agent_metadata(&self, session_id: &str, agent_id: &str) -> PathBuf {
        self.subagents_dir(session_id)
            .join(format!("agent-{agent_id}.meta.json"))
    }

    pub fn remote_agents_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("remote-agents")
    }

    pub fn remote_agent_metadata(&self, session_id: &str, task_id: &str) -> PathBuf {
        self.remote_agents_dir(session_id)
            .join(format!("remote-agent-{task_id}.meta.json"))
    }

    pub fn tool_results_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("tool-results")
    }

    pub fn session_memory_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session-memory")
    }

    pub fn session_memory_summary(&self, session_id: &str) -> PathBuf {
        self.session_memory_dir(session_id).join("summary.md")
    }

    pub fn session_usage(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("usage.json")
    }

    // ---- memory paths -------------------------------------------

    /// `<slug>/memory/`
    pub fn memory_dir(&self) -> PathBuf {
        self.project_dir().join("memory")
    }

    /// `<slug>/memory/MEMORY.md`
    pub fn memory_entrypoint(&self) -> PathBuf {
        self.memory_dir().join("MEMORY.md")
    }

    /// `<slug>/memory/team/`
    pub fn team_memory_dir(&self) -> PathBuf {
        self.memory_dir().join("team")
    }

    /// `<slug>/memory/team/MEMORY.md`
    pub fn team_memory_entrypoint(&self) -> PathBuf {
        self.team_memory_dir().join("MEMORY.md")
    }

    /// `<slug>/memory/.consolidate-lock`
    pub fn consolidation_lock(&self) -> PathBuf {
        self.memory_dir().join(".consolidate-lock")
    }

    /// `<slug>/memory/logs/YYYY/MM/YYYY-MM-DD.md`
    ///
    /// Components are zero-padded to match `Date.getFullYear/Month/Date`
    /// + `padStart(2,'0')` exactly.
    pub fn daily_log(&self, year: i32, month: u32, day: u32) -> PathBuf {
        let yyyy = format!("{year:04}");
        let mm = format!("{month:02}");
        let dd = format!("{day:02}");
        self.memory_dir()
            .join("logs")
            .join(&yyyy)
            .join(&mm)
            .join(format!("{yyyy}-{mm}-{dd}.md"))
    }
}

#[cfg(test)]
#[path = "project_paths.test.rs"]
mod tests;
