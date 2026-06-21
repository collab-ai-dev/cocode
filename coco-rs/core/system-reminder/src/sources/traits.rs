//! Per-category "source" traits for cross-crate reminder state.
//!
//! Each trait is implemented by the crate that owns the live state
//! (hooks, services/lsp, tasks, skills, services/mcp, app/state
//! swarm, bridge, memory). `coco-system-reminder` only defines the
//! trait shape and returns shared snapshot types; the owning crate
//! gains a dep on `coco-system-reminder` to `impl`, never the reverse.
//!
//! This is the reminder-subsystem analog of the "handle" traits in
//! `core/tool-runtime` (`AgentHandle` / `HookHandle` / `McpHandle` / …) —
//! same pattern: core crate defines the contract, upper crates
//! implement.
//!
//! Each method corresponds to a specific per-subsystem read that
//! the reminder orchestrator needs each turn.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;

use crate::generator::AgentPendingMessage;
use crate::generator::DiagnosticFileSummary;
use crate::generator::HookEvent;
use crate::generator::InvokedSkillEntry;
use crate::generator::TaskStatusSnapshot;
use crate::generator::TeamContextSnapshot;
use crate::generator::TeammateMailboxInfo;
use crate::generators::memory::NestedMemoryInfo;
use crate::generators::memory::RelevantMemoryInfo;
use crate::generators::user_input::IdeOpenedFileSnapshot;
use crate::generators::user_input::IdeSelectionSnapshot;
use crate::generators::user_input::McpResourceEntry;

/// Source of completed / pending async-hook responses.
///
/// Drains on read (marks delivered) — implementations should match:
/// `drain()` should NOT return the same event on a second call.
#[async_trait]
pub trait HookEventsSource: Send + Sync + Debug {
    /// Drain and return all newly-completed hook events since the
    /// last call. Implementations are expected to mark returned
    /// events delivered so they don't re-emit.
    async fn drain(&self, agent_id: Option<&str>) -> Vec<HookEvent>;
}

/// Source of new-since-last-snapshot LSP/IDE diagnostic entries.
///
/// Both IDE-reported and LSP-reported paths produce `diagnostics`
/// attachments; this single source collapses them.
#[async_trait]
pub trait DiagnosticsSource: Send + Sync + Debug {
    async fn snapshot(&self, agent_id: Option<&str>) -> Vec<DiagnosticFileSummary>;
}

/// Source of background-task status updates.
///
/// Primary use: post-compaction spawn-duplicate warning. The
/// `just_compacted` argument lets impls short-circuit unless a
/// compaction boundary was crossed this turn.
#[async_trait]
pub trait TaskStatusSource: Send + Sync + Debug {
    async fn collect(
        &self,
        agent_id: Option<&str>,
        just_compacted: bool,
    ) -> Vec<TaskStatusSnapshot>;
}

/// Source of skill listing + invoked-skill content.
///
/// `listing()` returns a pre-formatted string ready for injection;
/// `invoked()` returns per-skill content (name/path/body).
#[async_trait]
pub trait SkillsSource: Send + Sync + Debug {
    /// Per-turn skill listing reminder. `tiers` carries the
    /// `skill_overrides` resolution layer; impls must apply the 3-gate
    /// filter (`off` skip / `name-only` collapse / XG$) so the model
    /// only sees what the user permitted.
    async fn listing(
        &self,
        agent_id: Option<&str>,
        tiers: &coco_config::SkillOverrideTiers,
    ) -> Option<String>;
    async fn invoked(&self, agent_id: Option<&str>) -> Vec<InvokedSkillEntry>;
    /// Local keyword-match skill recommender. `tiers` is honoured so
    /// `off` skills never surface as recommendations.
    async fn skill_discovery(
        &self,
        _user_input: &str,
        _tiers: &coco_config::SkillOverrideTiers,
    ) -> Option<coco_types::SkillDiscoveryPayload> {
        None
    }
    async fn load_dynamic_skill_dir(
        &self,
        _skill_dir: &std::path::Path,
        _cwd: &std::path::Path,
    ) -> Option<coco_types::DynamicSkillPayload> {
        None
    }

    /// Activate path-gated skills whose `paths` frontmatter matches
    /// any of the given files (cwd-relative gitignore semantics).
    /// Returns names of newly-activated skills so callers can log.
    ///
    /// No default impl — every implementor must explicitly decide whether to
    /// support conditional activation; silently returning `Vec::new()` from a
    /// forgotten override would mask broken wiring.
    async fn activate_skills_for_paths(
        &self,
        file_paths: &[std::path::PathBuf],
        cwd: &std::path::Path,
    ) -> Vec<String>;
}

/// Source of MCP server state.
///
/// `instructions()` returns the current per-server instruction text
/// (engine diffs against app_state); `resolve_resources()` parses
/// @-mentions for MCP resource refs and resolves them to typed entries.
#[async_trait]
pub trait McpSource: Send + Sync + Debug {
    async fn instructions(&self, agent_id: Option<&str>) -> HashMap<String, String>;
    async fn resolve_resources(&self, agent_id: Option<&str>, input: &str)
    -> Vec<McpResourceEntry>;
}

/// Source of swarm / team state.
///
/// Gate: only call when `agentSwarms` is active upstream — impls
/// should also defensively return empty/None outside swarm sessions.
#[async_trait]
pub trait SwarmSource: Send + Sync + Debug {
    async fn teammate_mailbox(&self, agent_id: Option<&str>) -> Option<TeammateMailboxInfo>;
    async fn team_context(&self, agent_id: Option<&str>) -> Option<TeamContextSnapshot>;
    async fn agent_pending_messages(&self, agent_id: Option<&str>) -> Vec<AgentPendingMessage>;
}

/// Source of IDE bridge state (selection + opened file).
#[async_trait]
pub trait IdeBridgeSource: Send + Sync + Debug {
    async fn selection(&self, agent_id: Option<&str>) -> Option<IdeSelectionSnapshot>;
    async fn opened_file(&self, agent_id: Option<&str>) -> Option<IdeOpenedFileSnapshot>;
}

/// Source of memory-file reminder content.
///
/// `nested_memories()` resolves nested-CLAUDE.md traversal from
/// @-mention paths; `relevant_memories()` returns the ranked memory
/// set for the user's prompt text.
#[async_trait]
pub trait MemorySource: Send + Sync + Debug {
    /// Returns nested memory entries surfaced by @-mention traversal.
    /// Called with the set of @-mentioned paths (as a list of
    /// filesystem paths). Empty input → empty result.
    async fn nested_memories(
        &self,
        agent_id: Option<&str>,
        mentioned_paths: &[std::path::PathBuf],
    ) -> Vec<NestedMemoryInfo>;

    /// Returns the ranked relevant-memory set for `input` (user's
    /// prompt text this turn). Empty input → typically empty result.
    ///
    /// `recent_tools` lists tool names the model has used since the
    /// last user turn, fed into the LLM ranker so it can deprioritize
    /// tool reference docs the model is actively exercising. Empty slice
    /// when the engine has no signal yet.
    async fn relevant_memories(
        &self,
        agent_id: Option<&str>,
        input: &str,
        recent_tools: &[String],
    ) -> Vec<RelevantMemoryInfo>;
}

/// Bundle of optional source trait objects. `QueryEngine` holds one
/// `ReminderSources` field; CLI constructs it once at session start
/// by wiring each manager's trait impl.
///
/// Missing (`None`) fields → the corresponding reminder silently skips.
#[derive(Clone, Default)]
pub struct ReminderSources {
    pub hook_events: Option<Arc<dyn HookEventsSource>>,
    pub diagnostics: Option<Arc<dyn DiagnosticsSource>>,
    pub task_status: Option<Arc<dyn TaskStatusSource>>,
    pub skills: Option<Arc<dyn SkillsSource>>,
    pub mcp: Option<Arc<dyn McpSource>>,
    pub swarm: Option<Arc<dyn SwarmSource>>,
    pub ide: Option<Arc<dyn IdeBridgeSource>>,
    pub memory: Option<Arc<dyn MemorySource>>,
}

impl std::fmt::Debug for ReminderSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReminderSources")
            .field("hook_events", &self.hook_events.is_some())
            .field("diagnostics", &self.diagnostics.is_some())
            .field("task_status", &self.task_status.is_some())
            .field("skills", &self.skills.is_some())
            .field("mcp", &self.mcp.is_some())
            .field("swarm", &self.swarm.is_some())
            .field("ide", &self.ide.is_some())
            .field("memory", &self.memory.is_some())
            .finish()
    }
}
