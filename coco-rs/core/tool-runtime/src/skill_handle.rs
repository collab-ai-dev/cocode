//! Skill-resolution callback trait.
//!
//! Skills were historically reached via `AgentHandle::resolve_skill`,
//! but skills and agents are distinct runtime concepts — the
//! swarm-oriented agent handle was never the right home for skill
//! expansion. Phase 7 of the agent-loop refactor plan moves skill
//! resolution behind this dedicated trait.
//!
//! # Callback pattern
//!
//! `coco-skills` owns the `SkillManager` + expansion helpers but
//! lives above `coco-tool` in the layer graph; the trait lives here
//! and is implemented by `app/query` (or another high-layer adapter)
//! to bridge the call. This is the same pattern `HookHandle`,
//! `AgentHandle`, `MailboxHandle`, etc. use.
//!
//! # Invocation shapes
//!
//! Skills can be **inline** (expand prompt text into new user
//! messages) or **forked** (spawn a subagent). The trait returns a
//! typed [`SkillInvocationResult`]; the runtime decides which shape
//! to run based on the skill definition. A stub implementation
//! ([`NoOpSkillHandle`]) is provided for test/subagent contexts
//! where skill expansion is not supported.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;

use coco_config::SkillOverrideTiers;
use coco_types::ActiveShellTool;
use coco_types::Features;
use coco_types::PermissionMode;
use coco_types::PermissionUpdate;
use coco_types::ToolFilter;
use coco_types::ToolOverrides;

/// In-process subagent inheritance bundle.
///
/// Threaded from the parent `ToolUseContext` into [`SkillHandle::invoke_skill`]
/// (and [`crate::AgentSpawnRequest`]) so a forked subagent runs with
/// the same Layer 1 + Layer 2 + Layer 4 set as the parent. Subagents
/// only ever **narrow** these sets — never widen.
#[derive(Debug, Clone)]
pub struct SubagentInheritance {
    pub session_id: String,
    pub permission_mode: PermissionMode,
    pub features: Option<Arc<Features>>,
    pub tool_overrides: Option<Arc<ToolOverrides>>,
    pub active_shell_tool: ActiveShellTool,
    /// Parent's Layer 4 filter; the child's own `allowed_tools` /
    /// `disallowed_tools` is intersected with this via
    /// [`ToolFilter::narrow_with`].
    pub parent_tool_filter: Option<ToolFilter>,
}

impl Default for SubagentInheritance {
    fn default() -> Self {
        Self {
            session_id: "test-session".to_string(),
            permission_mode: PermissionMode::Default,
            features: None,
            tool_overrides: None,
            active_shell_tool: ActiveShellTool::Disabled,
            parent_tool_filter: None,
        }
    }
}

/// Per-invocation gate inputs for [`SkillHandle::invoke_skill`].
///
/// Two pieces the runtime needs to enforce the 4-state Skill tool
/// gate (`cli_inner_pretty.js:353567-353590`):
///
/// - [`Self::overrides`] — per-tier `skill_overrides` maps used by
///   `coco_skills::effective_skill_state` to compute the effective
///   state for this skill.
/// - [`Self::typed_slashes_in_turn`] — every `/<word>` token the user
///   typed in the current turn. The handle membership-checks
///   `(canonical_name, alias_1, alias_2, …)` so an aliased skill
///   like `commit-fast → commit` still bypasses the
///   `disable_model_invocation` author lock and the
///   `user-invocable-only` override.
#[derive(Debug, Clone, Default)]
pub struct SkillGateContext {
    pub overrides: Arc<SkillOverrideTiers>,
    pub typed_slashes_in_turn: std::collections::HashSet<String>,
}

/// Outcome of a skill invocation.
///
/// Inline skills return expanded messages that the runtime appends
/// to history (tagged with the parent tool_use_id). Forked skills
/// return the child agent's final text + metadata — the runtime
/// routes this through the same tool_result pipeline as `AgentTool`.
///
/// **Permission rules**: Inline variant carries `permission_updates`
/// the runner forwards on the `SkillTool`'s `ToolResult` so the
/// session's `QueryEngineConfig` picks up the skill's `allowed-tools`
/// frontmatter as `PermissionRuleSource::Command` auto-allow rules. The
/// Forked variant has no permission channel — fork-mode skills inject
/// their rules at dispatch time via `AgentQueryConfig.extra_permission_rules`
/// (Layer 4 inheritance), so by the time the subagent finishes there's
/// nothing to thread back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillInvocationResult {
    /// Expand the skill's prompt inline as new user messages.
    Inline {
        /// Pre-rendered summary of what ran, shown to the user in
        /// the tool result. Keep short — the detailed content goes
        /// into `new_messages`.
        summary: String,
        /// Messages to append to the agent's history. These are
        /// tagged with the parent tool_use_id upstream so skill
        /// outputs group with the SkillTool call that produced
        /// them.
        new_messages: Vec<serde_json::Value>,
        /// Permission-rule deltas the runner should forward on the
        /// `SkillTool`'s `ToolResult.permission_updates`. Built from
        /// the skill's `allowed-tools` frontmatter as
        /// `PermissionUpdate::AddRules { destination: Command }`. Empty
        /// when the skill has no `allowed-tools` declaration.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permission_updates: Vec<PermissionUpdate>,
    },
    /// Fork a subagent to run the skill and aggregate its output.
    Forked {
        /// Branded agent identifier so status queries / transcript
        /// lookups can address the child.
        agent_id: String,
        /// Final text aggregated from the child agent.
        output: String,
    },
}

/// Callback trait for resolving a skill by name and returning the
/// invocation outcome.
///
/// All methods are async and must be cancellation-aware. Long-running
/// forked skill execution should honor the parent turn's cancellation
/// token (threaded via `ToolUseContext`).
#[async_trait]
pub trait SkillHandle: Send + Sync {
    /// Resolve `name`, expand args against the skill definition, and
    /// run it in whatever invocation mode the skill declares
    /// (inline / forked).
    ///
    /// `args` is the raw string the model passed through the
    /// `SkillTool` — the handle is responsible for parsing it into
    /// the shape each skill expects.
    ///
    /// `inherit` carries the parent context's Layer 1 + Layer 2
    /// values for fork-mode skills; inline expansion ignores it.
    async fn invoke_skill(
        &self,
        name: &str,
        args: &str,
        inherit: SubagentInheritance,
        gate: SkillGateContext,
    ) -> Result<SkillInvocationResult, SkillInvocationError>;

    /// Read a skill's prompt body without invoking it. Used by
    /// `AgentTool` to preload skills declared in agent frontmatter
    /// (`skills: [foo, bar]`) — the bodies are prepended to the
    /// spawned subagent's first user message so the child sees the
    /// skill content from turn 1.
    ///
    /// **Skill-override gates apply.** Implementations must apply
    /// the same author / runtime gates as `invoke_skill` so a skill
    /// the user has hidden (`disable_model_invocation: true` or
    /// `skill_overrides == "off"`) cannot sneak into a subagent's
    /// preloaded prompt. The same predicate that gates the listing
    /// also gates skill commands.
    ///
    /// Default implementation returns `None` — handles that don't
    /// support preload (test fixtures / minimal embeddings) leave
    /// the skill body unloaded; the spawn proceeds with no preload.
    async fn read_skill_body(&self, _name: &str, _tiers: &SkillOverrideTiers) -> Option<String> {
        None
    }
}

/// Shared handle type for `ToolUseContext`.
pub type SkillHandleRef = Arc<dyn SkillHandle>;

/// Failure modes for skill invocation.
///
/// Intentionally closed so callers can match exhaustively and the
/// runner can map each case onto a stable model-visible error
/// result.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SkillInvocationError {
    #[error("skill not found: {name}")]
    NotFound { name: String },
    #[error("skill is disabled: {name}")]
    Disabled { name: String },
    #[error("skill is hidden from model: {name}")]
    HiddenFromModel { name: String },
    /// `skill_overrides` resolved to `off` — the model cannot invoke
    /// this skill at all. errorCode 7 (off branch).
    #[error("skill {name} is disabled for model invocation in skill_overrides settings")]
    OverrideOff { name: String },
    /// `skill_overrides` resolved to `user-invocable-only` and the
    /// user did not type `/<name>` in the current turn. errorCode
    /// 7 (user-invocable-only branch).
    #[error("skill {name} is user-invocable-only; type /{name} in the current turn to enable")]
    OverrideUserOnlyNoTrigger { name: String },
    #[error("argument expansion failed for {name}: {reason}")]
    Expansion { name: String, reason: String },
    #[error("forked skill execution failed: {reason}")]
    Forked { reason: String },
    #[error("remote skill invocation is not supported in this runtime")]
    RemoteUnsupported,
    #[error("skill runtime unavailable: {reason}")]
    Unavailable { reason: String },
}

/// No-op handle for contexts without a configured skill runtime
/// (unit tests, subagent sessions that inherit empty registries).
/// Every call returns `Unavailable`.
#[derive(Debug, Clone, Default)]
pub struct NoOpSkillHandle;

#[async_trait]
impl SkillHandle for NoOpSkillHandle {
    async fn invoke_skill(
        &self,
        _name: &str,
        _args: &str,
        _inherit: SubagentInheritance,
        _gate: SkillGateContext,
    ) -> Result<SkillInvocationResult, SkillInvocationError> {
        Err(SkillInvocationError::Unavailable {
            reason: "no skill runtime installed".into(),
        })
    }
}

#[cfg(test)]
#[path = "skill_handle.test.rs"]
mod tests;
