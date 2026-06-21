//! Auto-mode classification for permission decisions.
//!
//! Determines whether a tool can be used without prompting in auto mode.
//! Heuristic-based classifier with a safe-tool allowlist.

use coco_types::MCP_TOOL_PREFIX;
use coco_types::ToolName;

/// Result of auto-mode classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoModeDecision {
    /// Tool is safe to run without prompting.
    Allow,
    /// Tool needs user confirmation.
    NeedsPrompt { reason: String },
}

/// Extended input for auto-mode classification.
///
/// Callers construct this from tool metadata + shell analysis results.
pub struct AutoModeInput<'a> {
    pub tool_name: &'a str,
    pub input: &'a serde_json::Value,
    pub is_read_only: bool,
    /// For Bash tools: whether the command was classified as read-only
    /// by shell-level analysis (e.g., `is_read_only_command()`).
    pub bash_is_read_only: bool,
}

/// Classify whether a NON-file tool can be used in auto mode without the LLM
/// classifier.
///
/// File-modifying tools (Write/Edit/NotebookEdit/ApplyPatch) are **not**
/// handled here — the decision orchestrator
/// ([`crate::auto_mode_decision::can_use_tool_in_auto_mode`]) handles them
/// with path-safety + cwd context so a lexical "relative or /tmp" shortcut
/// can't auto-allow a CWD-escaping traversal or override a non-classifier-
/// approvable safety block.
///
/// Fast-allow set:
/// - All read-only tools: allow
/// - Bash: allow if command is read-only, otherwise prompt
/// - Task/Todo/Plan tools: allow (session-local side effects)
/// - Agent: allow — `AgentTool::is_read_only` is `true` (TS parity), so the
///   read-only fast path above handles it; the subagent's own tool calls are
///   gated under the inherited mode
/// - Unknown / network / team / scheduling / MCP / file tools: prompt
pub fn classify_for_auto_mode(
    tool_name: &str,
    input: &serde_json::Value,
    is_read_only: bool,
) -> AutoModeDecision {
    classify_auto_mode_extended(&AutoModeInput {
        tool_name,
        input,
        is_read_only,
        bash_is_read_only: false,
    })
}

/// Extended auto-mode classification with bash read-only awareness.
pub fn classify_auto_mode_extended(ctx: &AutoModeInput<'_>) -> AutoModeDecision {
    // Read-only tools are always safe
    if ctx.is_read_only {
        return AutoModeDecision::Allow;
    }

    let name = ctx.tool_name;

    // File-modifying tools are handled by the decision orchestrator with
    // path-safety + cwd context — never auto-allowed by a lexical shortcut
    // here. Defer to the classifier if one somehow reaches this path.
    if crate::evaluate::is_file_modifying_tool(name) {
        return AutoModeDecision::NeedsPrompt {
            reason: format!("{name} requires path-safety review"),
        };
    }

    // Bash: allow read-only commands, prompt for others
    if name == ToolName::Bash.as_str() {
        return if ctx.bash_is_read_only {
            AutoModeDecision::Allow
        } else {
            AutoModeDecision::NeedsPrompt {
                reason: "bash command requires review".into(),
            }
        };
    }

    // Task/Todo management tools — safe (session-local side effects only)
    const TASK_TOOLS: &[&str] = &[
        ToolName::TaskCreate.as_str(),
        ToolName::TaskUpdate.as_str(),
        ToolName::TaskGet.as_str(),
        ToolName::TaskList.as_str(),
        ToolName::TaskStop.as_str(),
        ToolName::TaskOutput.as_str(),
        ToolName::TodoWrite.as_str(),
    ];
    if TASK_TOOLS.contains(&name) {
        return AutoModeDecision::Allow;
    }

    // Plan mode tools — safe (local state changes)
    if name == ToolName::EnterPlanMode.as_str()
        || name == ToolName::ExitPlanMode.as_str()
        || name == ToolName::VerifyPlanExecution.as_str()
    {
        return AutoModeDecision::Allow;
    }

    // Team management — prompt (shared team-file / mailbox side effects).
    // `Agent` itself is intentionally NOT here: mirroring TS
    // `AgentTool.isReadOnly() => true`, an Agent spawn is treated as
    // read-only (it delegates permission checks to the subagent's own tool
    // calls), so it is auto-allowed before this list via the read-only fast
    // path in `classify_auto_mode_extended`.
    const TEAM_TOOLS: &[&str] = &[
        ToolName::SendMessage.as_str(),
        ToolName::TeamCreate.as_str(),
        ToolName::TeamDelete.as_str(),
    ];
    if TEAM_TOOLS.contains(&name) {
        return AutoModeDecision::NeedsPrompt {
            reason: format!("{name} mutates shared team state"),
        };
    }

    // Web tools — prompt (network access)
    if name == ToolName::WebFetch.as_str() || name == ToolName::WebSearch.as_str() {
        return AutoModeDecision::NeedsPrompt {
            reason: format!("{name} accesses the network"),
        };
    }

    // Worktree — prompt (git operations)
    if name == ToolName::EnterWorktree.as_str() || name == ToolName::ExitWorktree.as_str() {
        return AutoModeDecision::NeedsPrompt {
            reason: "worktree operations modify git state".into(),
        };
    }

    // Scheduling — prompt (persistent side effects)
    const SCHEDULE_TOOLS: &[&str] = &[
        ToolName::CronCreate.as_str(),
        ToolName::CronDelete.as_str(),
        ToolName::RemoteTrigger.as_str(),
    ];
    if SCHEDULE_TOOLS.contains(&name) {
        return AutoModeDecision::NeedsPrompt {
            reason: format!("{name} has persistent side effects"),
        };
    }

    // MCP tools — prompt (external system interaction)
    if name.starts_with(MCP_TOOL_PREFIX) {
        return AutoModeDecision::NeedsPrompt {
            reason: format!("MCP tool {name}"),
        };
    }

    // Unknown tools — prompt
    AutoModeDecision::NeedsPrompt {
        reason: format!("unknown tool: {name}"),
    }
}

#[cfg(test)]
#[path = "auto_mode.test.rs"]
mod tests;
