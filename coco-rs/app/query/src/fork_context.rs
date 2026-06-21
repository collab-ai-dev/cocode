//! Sub-context isolation primitives for fork-spawned engines.
//!
//! For every framework-spawned fork (promptSuggestion, sideQuestion,
//! compact, extractMemories, sessionMemory{Auto,Manual}, agentSummary,
//! autoDream, speculation), the parent's mutable `toolUseContext` state
//! is **cloned** (or fresh-started) so the child can't pollute the
//! parent — `readFileState`, `denialTrackingState`, `setAppState`
//! callbacks, the in-progress tool-use ID set, and the various trigger
//! sets all isolate.
//!
//! ## Why cloning matters for cache parity
//!
//! `readFileState` clone is **not** a no-op. Cache-shared forks
//! re-process the parent's `forkContextMessages` (which contain
//! tool_use_ids the parent already saw). A *fresh* `readFileState`
//! would treat those reads as unseen and trigger different
//! `<file_unchanged>` decisions, diverging the wire prefix and
//! breaking cache. A **clone** observes the same already-seen ids
//! ⇒ identical decisions ⇒ identical bytes ⇒ cache hit.
//!
//! ## NOT this module's job
//!
//! - Building the fork engine — `SessionRuntime::build_engine_from_config_with_fork`
//!   does that via `wire_engine` + per-call `ToolUseContext` overrides.
//! - Per-policy `canUseTool` callbacks — `coco-memory::can_use_tool`
//!   (auto-mem / session-mem) and `coco-query::speculation::boundary`
//!   (3-boundary overlay).

use std::path::PathBuf;

use coco_tool_runtime::CanUseToolHandleRef;
use coco_types::ForkLabel;

/// Fork-specific overrides for `SessionRuntime::wire_engine`.
///
/// Constructed by the dispatcher from a [`coco_query::forked_agent::ForkedAgentOptions`]
/// and threaded onto [`coco_query::QueryEngineConfig`] so the per-call
/// `ToolUseContext` builder can apply isolation uniformly across the
/// 8 fork variants.
///
/// Field defaults (via `Default`) are the **conservative isolation
/// shape** — most flags default to safe values, and callers only
/// flip them when the fork legitimately needs shared state (e.g.
/// `share_set_app_state=true` for an interactive subagent that
/// mutates parent UI state).
#[derive(Clone)]
pub struct ForkContextOverrides {
    /// Typed fork discriminator (used for telemetry + log fields).
    pub fork_label: ForkLabel,
    /// Free-form telemetry label (defaults to `fork_label.as_str()`
    /// via [`coco_query::forked_agent::ForkedAgentOptions::for_label`]).
    pub query_source: String,
    /// Per-fork agent id. `None` ⇒ auto-gen via [`auto_agent_id`].
    /// A fresh id is always allocated unless the caller pre-supplies one.
    pub agent_id: Option<String>,
    /// When `true` (default), the fork engine is built with a *deep clone*
    /// of the parent's `FileReadState` (see
    /// `SessionRuntime::build_engine_from_config_with_persistence`): the
    /// fork sees the parent's already-seen ids ⇒ identical
    /// `<file_unchanged>` decisions ⇒ cache parity, while its own
    /// reads/edits can't pollute the parent's dedup cache. Setting this
    /// `false` shares the parent's `Arc` (rare; breaks isolation).
    pub clone_file_read_state: bool,
    /// Per-fork canUseTool callback. Forwarded onto every
    /// `ToolUseContext.can_use_tool` so the tool-call preparer
    /// enforces the per-policy gate before static permission
    /// evaluation.
    pub can_use_tool: Option<CanUseToolHandleRef>,
    /// When `true`, hook auto-approve cannot bypass the
    /// `can_use_tool` callback. Speculation needs this so overlay
    /// path-rewrites always run.
    pub require_can_use_tool: bool,
    /// Memdir-only write fence (memory extract / dream / session
    /// services use this so the fork can only mutate inside the
    /// memdir). Empty = no fence. Enforces a path prefix via
    /// `ToolUseContext.allowed_write_roots`.
    pub allowed_write_roots: Vec<PathBuf>,
    /// Parent's query-tracking depth. The fork's own depth is
    /// `parent_query_depth + 1`; increments through nested subagents.
    pub parent_query_depth: i32,
}

impl std::fmt::Debug for ForkContextOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkContextOverrides")
            .field("fork_label", &self.fork_label)
            .field("query_source", &self.query_source)
            .field("agent_id", &self.agent_id)
            .field("clone_file_read_state", &self.clone_file_read_state)
            .field("can_use_tool_set", &self.can_use_tool.is_some())
            .field("require_can_use_tool", &self.require_can_use_tool)
            .field("allowed_write_roots", &self.allowed_write_roots)
            .field("parent_query_depth", &self.parent_query_depth)
            .finish()
    }
}

impl ForkContextOverrides {
    /// Build the conservative isolation shape for `label`.
    ///
    /// Defaults for the fire-and-forget side-channel case:
    /// - `clone_file_read_state = true` (per-fork dedup-cache isolation)
    /// - `agent_id = None` (auto-gen)
    /// - `require_can_use_tool = false` (auto-approve hooks
    ///   bypass; speculation overrides to `true`)
    ///
    /// Cancellation tokens flow through
    /// [`coco_query::forked_agent::ForkedAgentOptions::overrides::abort`]
    /// (the dispatcher level) — not through this struct — to keep
    /// the abort plumbing single-pathed.
    pub fn for_label(label: ForkLabel) -> Self {
        Self {
            query_source: label.as_str().to_string(),
            fork_label: label,
            agent_id: None,
            clone_file_read_state: true,
            can_use_tool: None,
            require_can_use_tool: false,
            allowed_write_roots: Vec::new(),
            parent_query_depth: 0,
        }
    }

    /// Compute the depth this fork should use on its
    /// `ToolUseContext.query_depth` field — parent depth + 1, with
    /// a sanity cap at 16 to prevent runaway recursion in
    /// fork-spawning-fork scenarios.
    pub fn child_query_depth(&self) -> i32 {
        const MAX_DEPTH: i32 = 16;
        (self.parent_query_depth + 1).min(MAX_DEPTH)
    }
}

/// Auto-generate an agent id for an unowned fork. Format:
/// `fork-<label>-<uuid>` so log readers can grep both the variant
/// and the run.
pub fn auto_agent_id(label: ForkLabel) -> String {
    format!("fork-{}-{}", label.as_str(), uuid::Uuid::new_v4())
}

#[cfg(test)]
#[path = "fork_context.test.rs"]
mod tests;
