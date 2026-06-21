use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use crate::PermissionMode;

/// Permission behavior for a rule.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

/// Classifier behavior (same variants as `PermissionBehavior`).
pub type ClassifierBehavior = PermissionBehavior;

/// Which auto-mode classifier stages run. TS `TwoStageMode`
/// (`yoloClassifier.ts:1308`).
///
/// Controls only *which stages execute and their token budgets* — never the
/// model. Every mode runs on `ModelRole::Main`, mirroring TS running every
/// mode on `getMainLoopModel()`. Shared between `coco-config`
/// (`AutoModeConfig`) and `coco-permissions` (`AutoModeRules`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifierMode {
    /// Stage 1 fast (64 tok, stop `</block>`) → escalate to Stage 2 (4096 tok)
    /// on block / unparseable. The default.
    #[default]
    Both,
    /// Single fast stage: 256 tok, no stop sequence, verdict final. TS `fast`.
    Fast,
    /// Stage 2 only: 4096 tok, no stop sequence. TS `thinking`.
    Thinking,
}

/// Token usage from the classifier.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifierUsage {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
}

/// Pending classifier check — captures context for deferred bash classification.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClassifierCheck {
    pub command: String,
    pub cwd: String,
    #[serde(default)]
    pub descriptions: Vec<String>,
}

/// Source of a permission rule (ordered by priority: Session is most specific).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleSource {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    FlagSettings,
    PolicySettings,
    CliArg,
    Command,
    Session,
}

// ── Shared rule matching functions ──
// Used by both coco-permissions (rule evaluation) and coco-hooks (if condition).

/// Parse a rule string like `"Bash(git *)"` into tool_pattern + rule_content.
///
/// This is a simplified parser for the hook `if` condition syntax. For the
/// full-featured parser with escape handling, see `coco-permissions::rule_compiler`.
pub fn parse_rule_pattern(rule_str: &str) -> PermissionRuleValue {
    if let Some(open) = rule_str.find('(')
        && let Some(close) = rule_str.rfind(')')
        && close > open
        && close == rule_str.len() - 1
    {
        let tool = &rule_str[..open];
        let content = &rule_str[open + 1..close];
        if !tool.is_empty() && !content.is_empty() && content != "*" {
            return PermissionRuleValue {
                tool_pattern: tool.to_string(),
                rule_content: Some(content.to_string()),
            };
        }
        return PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: None,
        };
    }
    PermissionRuleValue {
        tool_pattern: rule_str.to_string(),
        rule_content: None,
    }
}

/// Check if a tool name matches a rule's tool pattern.
///
/// Supports: exact match, `"*"` wildcard, prefix-wildcard (`"mcp__slack__*"`),
/// and MCP server-level matching (`"mcp__server"` matches `"mcp__server__tool"`).
pub fn tool_matches_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }

    if pattern == tool_name {
        return true;
    }

    // MCP server-level match: rule "mcp__server1" matches "mcp__server1__tool1"
    if pattern.starts_with(crate::MCP_TOOL_PREFIX) && tool_name.starts_with(crate::MCP_TOOL_PREFIX)
    {
        let rule_parts: Vec<&str> = pattern.splitn(3, "__").collect();
        let tool_parts: Vec<&str> = tool_name.splitn(3, "__").collect();
        if rule_parts.len() == 2 && tool_parts.len() == 3 && rule_parts[1] == tool_parts[1] {
            return true;
        }
    }

    false
}

/// Check if tool content matches a rule's content pattern.
///
/// Supports prefix matching (ending with `*`) and exact matching.
pub fn content_matches(rule_content: &str, tool_content: &str) -> bool {
    if rule_content == "*" {
        return true;
    }

    if let Some(prefix) = rule_content.strip_suffix('*') {
        return tool_content.starts_with(prefix);
    }

    rule_content == tool_content
}

/// Check if a tool call matches a parsed rule pattern.
///
/// Combines `tool_matches_pattern` and `content_matches` for convenience.
pub fn matches_rule(
    rule: &PermissionRuleValue,
    tool_name: &str,
    tool_content: Option<&str>,
) -> bool {
    if !tool_matches_pattern(&rule.tool_pattern, tool_name) {
        return false;
    }
    match (&rule.rule_content, tool_content) {
        (Some(rc), Some(tc)) => content_matches(rc, tc),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Permission rule value — tool_pattern is a glob/wildcard expression.
/// Examples: "Read", "Bash(git *)", "mcp__slack__*", "*"
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRuleValue {
    pub tool_pattern: String,
    /// Command pattern within tool (e.g. "git *").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

/// A single permission rule.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub source: PermissionRuleSource,
    pub behavior: PermissionBehavior,
    pub value: PermissionRuleValue,
}

/// Why a permission decision was made.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionDecisionReason {
    Rule {
        rule: PermissionRule,
    },
    Mode {
        mode: PermissionMode,
    },
    Classifier {
        classifier: String,
        reason: String,
    },
    Hook {
        hook_name: String,
        reason: Option<String>,
    },
    SafetyCheck {
        reason: String,
        classifier_approvable: bool,
    },
    AsyncAgent {
        reason: String,
    },
    User,
    Sandboxed,
}

/// Result of a tool's own permission opinion (the step-1c slot in
/// the central evaluator pipeline).
///
/// `tool.checkPermissions()` returns
/// `{ behavior: 'allow' | 'ask' | 'deny', updatedInput?, feedback? }`
/// or is absent (== passthrough). `Passthrough` is the explicit
/// "no opinion — defer to rule pipeline" signal. Tools that don't
/// implement content-specific safety checks return `Passthrough`.
///
/// Lives in `coco-types` rather than `coco-permissions` so the
/// `coco_tool_runtime::Tool::check_permissions` trait method can
/// reference it without the L4 Tool trait depending on the L3
/// permissions evaluator.
#[derive(Debug, Clone)]
pub enum ToolCheckResult {
    /// Tool has no opinion — continue with rule-based checks.
    Passthrough,
    /// Tool explicitly allows this input. `updated_input` carries
    /// any normalization the tool applied (TS `updatedInput`);
    /// `feedback` carries an optional user-facing rationale that
    /// the evaluator threads onto the resulting `PermissionDecision`.
    Allow {
        updated_input: Option<serde_json::Value>,
        feedback: Option<String>,
    },
    /// Tool requires user confirmation for this input.
    ///
    /// `choices` is `None` for the traditional yes/no dialog. When
    /// `Some`, the TUI renders a multi-choice list instead and the
    /// selected `value` is echoed back to the tool via
    /// `PermissionResolutionDetail` so `execute()` can branch on it.
    /// TS parity: `ExitPlanModePermissionRequest.tsx:691-704` option grid.
    Ask {
        message: String,
        /// Permission updates the frontend may apply when the user picks
        /// "always allow".
        suggestions: Vec<PermissionUpdate>,
        choices: Option<Vec<PermissionAskChoice>>,
        detail: Option<PermissionRequestDetail>,
    },
    /// Tool denies this input.
    Deny { message: String },
}

/// Why a permission flow aborted the current turn instead of returning a
/// normal model-visible denial.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAbortReason {
    UserAbort,
    ClassifierTranscriptTooLong,
    ClassifierDenialLimit,
    PermissionRequestCancelled,
}

/// One option in a multi-choice permission dialog.
///
/// Used by `ToolCheckResult::Ask.choices` and surfaced on the wire via
/// `PermissionDecision::Ask.choices`. The TUI renders one row per
/// choice; the picked `value` is echoed back so the tool's `execute()`
/// can branch on the user's selection.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAskChoice {
    /// Stable identifier echoed back in the approval response. Use
    /// kebab-case (`"yes-default-keep-context"`,
    /// `"yes-accept-edits-clear-context"`, `"no"`).
    pub value: String,
    /// Short row label shown to the user.
    pub label: String,
    /// Optional one-line explanation rendered under the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Tool-specific payload for permission UIs.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionRequestDetail {
    ExitPlanMode {
        outcome: ExitPlanModeOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_file_path: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_prompts: Vec<ExitPlanModeAllowedPrompt>,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExitPlanModeAllowedPrompt {
    pub tool: String,
    pub prompt: String,
}

/// The user's response to an `ExitPlanMode` approval prompt.
///
/// The wire `value` strings are the single source of truth for the choice
/// echoed back through `PermissionDecision::Ask.choices` →
/// `PermissionResolutionDetail::ExitPlanMode.choice`. Owning the mapping here keeps
/// the producer (the TUI permission bridge, which builds the choice list) and
/// the consumer (`ExitPlanModeTool::execute`, which branches on the picked
/// value) from drifting apart.
///
/// Adapted from TS `ExitPlanModePermissionRequest.tsx` `ResponseValue` union.
/// The two clear-context variants intentionally diverge from the TS strings
/// (`yes-accept-edits` / `yes-bypass-permissions`): coco appends an explicit
/// `-clear-context` suffix so every clear variant reads symmetrically against
/// the `-keep-context` keep variants — self-documenting in logs and on the wire.
/// The value is ephemeral (resolved to the enum within a turn, never persisted),
/// so the rename is a clean break with no legacy alias.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitPlanChoice {
    /// Clear context, then implement with permissions bypassed.
    #[serde(rename = "yes-bypass-permissions-clear-context")]
    ClearBypassPermissions,
    /// Clear context, then implement auto-accepting edits.
    #[serde(rename = "yes-accept-edits-clear-context")]
    ClearAcceptEdits,
    /// Keep context; auto-accept edits (or bypass when the gate allows).
    #[serde(rename = "yes-accept-edits-keep-context")]
    KeepAcceptEdits,
    /// Keep context; restore the pre-plan mode (default → manual approval).
    #[serde(rename = "yes-default-keep-context")]
    KeepDefault,
    /// Reject the plan and stay in plan mode. Never reaches `execute` (the
    /// TUI maps it to a denial) — carried so the bridge and the "is this a
    /// rejection?" check share one wire constant.
    #[serde(rename = "no")]
    No,
}

impl ExitPlanChoice {
    /// Stable wire value echoed back in the approval response.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClearBypassPermissions => "yes-bypass-permissions-clear-context",
            Self::ClearAcceptEdits => "yes-accept-edits-clear-context",
            Self::KeepAcceptEdits => "yes-accept-edits-keep-context",
            Self::KeepDefault => "yes-default-keep-context",
            Self::No => "no",
        }
    }

    /// Parse a wire value back into a choice; `None` for an unrecognized value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "yes-bypass-permissions-clear-context" => Some(Self::ClearBypassPermissions),
            "yes-accept-edits-clear-context" => Some(Self::ClearAcceptEdits),
            "yes-accept-edits-keep-context" => Some(Self::KeepAcceptEdits),
            "yes-default-keep-context" => Some(Self::KeepDefault),
            "no" => Some(Self::No),
            _ => None,
        }
    }

    /// Whether this choice clears conversation context before implementing.
    pub const fn clears_context(self) -> bool {
        matches!(self, Self::ClearBypassPermissions | Self::ClearAcceptEdits)
    }
}

/// Tool-specific trusted metadata attached to a permission approval.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionResolutionDetail {
    ExitPlanMode {
        choice: ExitPlanChoice,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edited_plan: Option<String>,
    },
}

/// First-class outcome of an `ExitPlanMode` tool call.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitPlanModeOutcome {
    /// A concrete implementation plan was produced and approved for execution.
    #[default]
    ImplementationPlan,
    /// Plan mode ended for a request that does not have implementation work.
    NoImplementationPlan,
}

impl ExitPlanModeOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImplementationPlan => "implementation_plan",
            Self::NoImplementationPlan => "no_implementation_plan",
        }
    }

    pub const fn has_implementation_plan(self) -> bool {
        matches!(self, Self::ImplementationPlan)
    }
}

/// The result of a permission check.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
    Ask {
        message: String,
        #[serde(default)]
        suggestions: Vec<PermissionUpdate>,
        /// Optional multi-choice payload. When `Some`, the TUI renders
        /// a choice list instead of yes/no; the picked `value` is sent
        /// back to the tool. TS parity:
        /// `ExitPlanModePermissionRequest.tsx:691-704` option grid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        choices: Option<Vec<PermissionAskChoice>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<PermissionRequestDetail>,
    },
    Deny {
        message: String,
        reason: PermissionDecisionReason,
    },
    Abort {
        message: String,
        reason: PermissionAbortReason,
    },
}

/// A permission update action.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionUpdate {
    AddRules {
        rules: Vec<PermissionRule>,
        destination: PermissionUpdateDestination,
    },
    ReplaceRules {
        rules: Vec<PermissionRule>,
        destination: PermissionUpdateDestination,
    },
    RemoveRules {
        rules: Vec<PermissionRule>,
        destination: PermissionUpdateDestination,
    },
    SetMode {
        mode: PermissionMode,
    },
    AddDirectories {
        directories: Vec<String>,
        destination: PermissionUpdateDestination,
    },
    RemoveDirectories {
        directories: Vec<String>,
        destination: PermissionUpdateDestination,
    },
}

impl PermissionUpdate {
    /// Destination of this update, if any. `SetMode` has no destination
    /// (it changes session state, not a settings layer).
    pub const fn destination(&self) -> Option<PermissionUpdateDestination> {
        match self {
            Self::AddRules { destination, .. }
            | Self::ReplaceRules { destination, .. }
            | Self::RemoveRules { destination, .. }
            | Self::AddDirectories { destination, .. }
            | Self::RemoveDirectories { destination, .. } => Some(*destination),
            Self::SetMode { .. } => None,
        }
    }
}

/// Destination for persisting permission updates.
///
/// Persistable destinations (`User`/`Project`/`LocalSettings`) write to
/// disk; in-memory destinations (`Session`/`CliArg`/`Command`) live only
/// for the running session. TS parity: same split as
/// `persistPermissionUpdates` in `PermissionUpdate.ts`.
///
/// `Command` is reserved for rules contributed by an invoked command or
/// skill's frontmatter (`allowed-tools:`). TS parity:
/// `alwaysAllowRules.command` populated by `SkillTool` /
/// `createGetAppStateWithAllowedTools`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionUpdateDestination {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    Session,
    CliArg,
    /// Rules contributed by a command/skill frontmatter `allowed-tools`.
    /// In-memory only; cleared on session end.
    Command,
}

/// Rules grouped by source (for ToolPermissionContext).
pub type PermissionRulesBySource = HashMap<PermissionRuleSource, Vec<PermissionRule>>;

/// Source of a working directory addition.
pub type WorkingDirectorySource = PermissionUpdateDestination;

/// Additional working directory info for permission evaluation. Tracks source origin.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalWorkingDir {
    pub path: String,
    /// Where this directory was configured from.
    pub source: WorkingDirectorySource,
}

/// Context for evaluating tool permissions.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionContext {
    pub mode: PermissionMode,
    #[serde(default)]
    pub additional_dirs: HashMap<String, AdditionalWorkingDir>,
    /// Source-specific roots for path-scoped file permission rules.
    ///
    /// TS parity: `rootPathForSource()` in `utils/permissions/filesystem.ts`.
    /// Empty falls back to cwd-derived roots for test contexts.
    #[serde(default)]
    pub permission_rule_source_roots: HashMap<PermissionRuleSource, std::path::PathBuf>,
    #[serde(default)]
    pub allow_rules: PermissionRulesBySource,
    #[serde(default)]
    pub deny_rules: PermissionRulesBySource,
    #[serde(default)]
    pub ask_rules: PermissionRulesBySource,
    #[serde(default)]
    pub bypass_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_plan_mode: Option<PermissionMode>,
    /// Rules stashed during auto-mode entry (dangerous classifier-bypass rules).
    /// Restored on auto-mode exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripped_dangerous_rules: Option<PermissionRulesBySource>,
    /// Pre-resolved session plan file path — pushed in by the engine so
    /// the permission evaluator can auto-allow writes to it in Plan mode
    /// without re-deriving the slug. TS parity: `isSessionPlanFile` in
    /// `utils/permissions/filesystem.ts:245-255`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_plan_file: Option<std::path::PathBuf>,
}
