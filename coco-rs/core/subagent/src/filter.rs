//! Tool filtering and nested-agent restriction parsing.
//!
//! Pure logic: the universal subagent deny-list ([`subagent_disallowed_tools`]),
//! allow-list normalisation ([`parse_tool_allow_list`]), and `Agent(...)` /
//! `Task(...)` permission-entry parsing ([`parse_allowed_agent_types`]). The
//! coordinator spawn path consumes these to build the child `ToolFilter`; this
//! crate never touches the registry.

use coco_types::ToolName;
use strum::IntoEnumIterator;

/// Tools blocked for every spawned agent.
///
/// Note: internally `Agent` can be re-allowed for ant builds to enable
/// nested-agent recursion. The default 3P/SDK build keeps `Agent` blocked;
/// the runtime can override the list per-spawn for ant builds.
pub const ALL_AGENT_DISALLOWED_TOOLS: &[&str] = &[
    ToolName::TaskOutput.as_str(),
    ToolName::ExitPlanMode.as_str(),
    ToolName::EnterPlanMode.as_str(),
    ToolName::Agent.as_str(),
    ToolName::AskUserQuestion.as_str(),
    ToolName::TaskStop.as_str(),
];

/// Tools that are safe inside a background (async) agent.
///
/// Shell tools include `Bash` and `PowerShell` only — REPL is intentionally
/// excluded from the async-safe set (REPL is a long-lived stateful process
/// the runtime can't safely background).
pub const ASYNC_AGENT_ALLOWED_TOOLS: &[&str] = &[
    ToolName::Read.as_str(),
    ToolName::WebSearch.as_str(),
    ToolName::TodoWrite.as_str(),
    ToolName::Grep.as_str(),
    ToolName::WebFetch.as_str(),
    ToolName::Glob.as_str(),
    ToolName::Bash.as_str(),
    ToolName::PowerShell.as_str(),
    ToolName::Edit.as_str(),
    ToolName::Write.as_str(),
    ToolName::NotebookEdit.as_str(),
    ToolName::Skill.as_str(),
    ToolName::StructuredOutput.as_str(),
    ToolName::ToolSearch.as_str(),
    ToolName::EnterWorktree.as_str(),
    ToolName::ExitWorktree.as_str(),
];

/// The universal subagent tool block as deny-list names — the tools every
/// spawned subagent is denied regardless of its allow-list. `ExitPlanMode`
/// is re-admitted when `plan_mode` so a plan-mode subagent can still exit
/// the plan.
///
/// coco-rs enforces tool visibility per-id via
/// [`coco_types::ToolFilter::allows`] (`tool-runtime/registry.rs`), so a
/// deny entry simply drops that tool from the model's list — no
/// `available_tools` snapshot is required. The caller (coordinator spawn
/// path) merges these into the child `ToolFilter`'s disallowed set.
pub fn subagent_disallowed_tools(plan_mode: bool) -> Vec<&'static str> {
    let exit_plan_mode = ToolName::ExitPlanMode.as_str();
    ALL_AGENT_DISALLOWED_TOOLS
        .iter()
        .copied()
        .filter(|name| !(plan_mode && *name == exit_plan_mode))
        .collect()
}

/// Deny-list that clamps a background (async) subagent to the async-safe
/// tool set — every built-in tool NOT in [`ASYNC_AGENT_ALLOWED_TOOLS`].
///
/// TS parity: `filterToolsForAgent` (`agentToolUtils.ts`) strips every
/// tool outside the async-safe set when `isAsync`
/// (`run_in_background || agent.background`). REPL and other long-lived
/// stateful tools the runtime can't safely background are excluded.
///
/// MCP tools (`mcp__*`) are NOT [`ToolName`] variants, so they never
/// appear here and pass through — mirroring TS, where MCP bypasses the
/// async clamp. `ExitPlanMode` is re-admitted in plan mode so a
/// plan-mode background spawn can still exit the plan. The caller merges
/// these into the child `ToolFilter`'s disallowed set (deny wins over the
/// definition's own allow-list, exactly like the universal block).
pub fn async_subagent_disallowed_tools(plan_mode: bool) -> Vec<&'static str> {
    let exit_plan_mode = ToolName::ExitPlanMode.as_str();
    ToolName::iter()
        .map(|t| t.as_str())
        .filter(|name| !ASYNC_AGENT_ALLOWED_TOOLS.contains(name))
        .filter(|name| !(plan_mode && *name == exit_plan_mode))
        .collect()
}

/// Strip parenthesized arguments from allow-list entries: `Bash(*)` ↦ `Bash`.
///
/// Public so the subagent spawn path can normalise an
/// `AgentDefinition.allowed_tools` `Explicit` list into bare tool names
/// before handing them to a `ToolFilter` (which matches by `ToolId`, so a
/// raw `Bash(*)` would parse to `Custom("Bash(*)")` and never match).
pub fn parse_tool_allow_list(items: &[String]) -> Vec<&str> {
    items
        .iter()
        .map(|s| match s.find('(') {
            Some(i) => s[..i].trim(),
            None => s.trim(),
        })
        .collect()
}

// ── AllowedAgentTypes ──

/// Parsed `Agent(type1, type2, ...)` / `Task(type1, type2, ...)` restriction
/// from a permission rule. Both tool names are accepted (`Task` is a
/// permanent alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedAgentTypes {
    pub names: Vec<String>,
}

impl AllowedAgentTypes {
    /// A parsed entry with no listed types means the rule did not restrict
    /// types (it was effectively `Agent` or `Agent()` in the user's
    /// permissions), so every agent_type is allowed.
    pub fn matches(&self, agent_type: &str) -> bool {
        self.names.is_empty() || self.names.iter().any(|n| n == agent_type)
    }
}

/// Parse one allow-list entry like `Agent(Explore,plan)` or `Task(Plan)`.
///
/// Returns:
/// - `None` if the entry is not an `Agent`/`Task` restriction at all
///   (e.g. `Bash(npm test)` — caller should ignore those for this purpose).
/// - `None` for bare `Agent` / `Agent()` — the runtime treats this as
///   "no restriction"; returning `None` lets callers skip the matching
///   step entirely. To match this with a parsed value, use
///   `AllowedAgentTypes { names: vec![] }` whose `matches()` returns true
///   for every agent_type.
/// - `Some(AllowedAgentTypes { names })` with the listed types when an
///   explicit list is given (e.g. `Agent(Explore,Plan)`).
pub fn parse_allowed_agent_types(rule: &str) -> Option<AllowedAgentTypes> {
    let trimmed = rule.trim();
    let (head, paren_body) = match trimmed.find('(') {
        Some(i) => (&trimmed[..i], Some(&trimmed[i + 1..])),
        None => (trimmed, None),
    };
    if head.trim() != "Agent" && head.trim() != "Task" {
        return None;
    }
    let Some(body) = paren_body else {
        // Bare `Agent` / `Task` — no restriction. Caller treats as unrestricted.
        return None;
    };
    let inner = body.trim_end_matches(')');
    let names: Vec<String> = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if names.is_empty() {
        // `Agent()` with empty parens — also unrestricted.
        return None;
    }
    Some(AllowedAgentTypes { names })
}
