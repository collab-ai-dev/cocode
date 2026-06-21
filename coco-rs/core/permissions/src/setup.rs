//! Permission setup — initial mode selection, mode descriptions, default rule
//! generation, and configuration validation.

use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolName;

use crate::rule_compiler;

/// Compute whether the classifier-backed `Auto` permission mode is available
/// for this session (the Shift+Tab cycle gate + control-plane entry gate).
///
/// Parallel to [`crate::compute_bypass_capability`]. coco-rs intentionally
/// drops TS's three-part gate (`feature('TRANSCRIPT_CLASSIFIER')` +
/// `tengu_auto_mode_config` GrowthBook circuit breaker + `modelSupportsAutoMode`
/// allow-list): there is no GrowthBook here and the classifier subsystem ships
/// for every provider/model. Auto is therefore **available by default**, gated
/// only by the `auto_mode.disabled` settings opt-out (mirrors TS
/// `disableAutoMode`). A future model-support check would slot in here.
pub fn compute_auto_mode_capability(settings_disabled: bool) -> bool {
    !settings_disabled
}

// ��─ PermissionModeChoice ──

/// User-facing permission mode selection during onboarding or mode-switch.
///
/// Maps to `PermissionMode` but uses names that are clearer in an interactive
/// selection dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionModeChoice {
    /// Standard interactive mode — ask before risky operations.
    Interactive,
    /// Auto-approve all safe operations, only ask for risky ones.
    Auto,
    /// Plan-only mode — no writes, useful for reviewing strategy.
    Plan,
    /// Bypass all prompts (for sandboxed environments).
    Bypass,
}

impl PermissionModeChoice {
    /// Convert to the underlying `PermissionMode`.
    pub fn to_permission_mode(self) -> PermissionMode {
        match self {
            Self::Interactive => PermissionMode::Default,
            Self::Auto => PermissionMode::Auto,
            Self::Plan => PermissionMode::Plan,
            Self::Bypass => PermissionMode::BypassPermissions,
        }
    }

    /// Human-readable label for this choice (for TUI selection lists).
    pub fn label(self) -> &'static str {
        match self {
            Self::Interactive => "Interactive (recommended)",
            Self::Auto => "Auto mode (classifier-assisted)",
            Self::Plan => "Plan mode (read-only)",
            Self::Bypass => "Bypass permissions (sandboxed only)",
        }
    }

    /// Detailed description for this choice.
    pub fn description(self) -> &'static str {
        match self {
            Self::Interactive => {
                "You approve each tool use that can modify your system. Read-only tools are auto-approved."
            }
            Self::Auto => {
                "An AI classifier evaluates each tool call. Safe operations are auto-approved; \
                 risky ones still prompt for confirmation."
            }
            Self::Plan => {
                "Claude plans changes but does not execute them. Switch to a write mode \
                 after reviewing the strategy."
            }
            Self::Bypass => {
                "All tool calls are auto-approved. Only use in sandboxed or throw-away \
                 environments."
            }
        }
    }

    /// All choices in display order.
    pub const ALL: &[Self] = &[Self::Interactive, Self::Auto, Self::Plan, Self::Bypass];
}

// ── Mode descriptions ──

/// Human-readable title for a permission mode (used in TUI, CLI, and prompts).
pub fn permission_mode_title(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "Default",
        PermissionMode::Plan => "Plan Mode",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::BypassPermissions => "Bypass Permissions",
        PermissionMode::DontAsk => "Don't Ask",
        PermissionMode::Auto => "Auto mode",
        PermissionMode::Bubble => "Bubble",
    }
}

/// Abbreviated mode title for compact displays (status bar, badges).
pub fn permission_mode_short_title(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "Default",
        PermissionMode::Plan => "Plan",
        PermissionMode::AcceptEdits => "Accept",
        PermissionMode::BypassPermissions => "Bypass",
        PermissionMode::DontAsk => "DontAsk",
        PermissionMode::Auto => "Auto",
        PermissionMode::Bubble => "Bubble",
    }
}

/// Mode selection description shown during onboarding / mode-switch dialogs.
pub fn permission_mode_description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => {
            "You will be asked to approve each tool use that could modify your system. \
             Read-only tools are auto-approved."
        }
        PermissionMode::Plan => {
            "Claude will plan changes but not execute them. \
             Use this to review a strategy before switching to a write-capable mode."
        }
        PermissionMode::AcceptEdits => {
            "File edits (Read, Edit, Write, Glob, Grep) are auto-approved. \
             Shell commands and other tools still require approval."
        }
        PermissionMode::BypassPermissions => {
            "All tool calls are auto-approved. Use only in sandboxed or \
             throw-away environments — this disables all safety prompts."
        }
        PermissionMode::DontAsk => {
            "All tool calls are auto-approved and no permission prompts are shown. \
             Similar to Bypass but also suppresses informational prompts."
        }
        PermissionMode::Auto => {
            "An AI classifier evaluates each tool call. Safe operations are \
             auto-approved; risky ones still prompt for confirmation."
        }
        PermissionMode::Bubble => {
            "Permission decisions are escalated to the parent agent. \
             Used internally for sub-agents."
        }
    }
}

/// Whether a mode is the default mode (or unset).
pub fn is_default_mode(mode: Option<PermissionMode>) -> bool {
    matches!(mode, None | Some(PermissionMode::Default))
}

// ── Dangerous permission detection ──

/// Interpreters and runners dangerous on both Bash and PowerShell.
const CROSS_PLATFORM_CODE_EXEC: &[&str] = &[
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl", "php", "lua", "npx",
    "bunx", "npm run", "yarn run", "pnpm run", "bun run", "bash", "sh", "ssh",
];

/// Dangerous bash patterns: cross-platform interpreters + bash-specific shells/evaluators.
const DANGEROUS_BASH_PATTERNS: &[&str] = &[
    // Cross-platform (shared with PowerShell)
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl", "php", "lua", "npx",
    "bunx", "npm run", "yarn run", "pnpm run", "bun run", "bash", "sh", "ssh",
    // Bash-specific
    "zsh", "fish", "eval", "exec", "env", "xargs", "sudo",
];

/// Additional patterns only dangerous for Anthropic-internal users.
const DANGEROUS_BASH_PATTERNS_ANT_ONLY: &[&str] = &[
    "fa run", "coo", "gh", "gh api", "curl", "wget", "git", "kubectl", "aws", "gcloud", "gsutil",
];

/// PowerShell-specific dangerous patterns (on top of CROSS_PLATFORM_CODE_EXEC).
const DANGEROUS_POWERSHELL_PATTERNS: &[&str] = &[
    // Nested PS + shells
    "pwsh",
    "powershell",
    "cmd",
    "wsl",
    // String/scriptblock evaluators
    "iex",
    "invoke-expression",
    "icm",
    "invoke-command",
    // Process spawners
    "start-process",
    "saps",
    "start",
    "start-job",
    "sajb",
    "start-threadjob",
    // Event/session code exec
    "register-objectevent",
    "register-engineevent",
    "register-wmievent",
    "register-scheduledjob",
    "new-pssession",
    "nsn",
    "enter-pssession",
    "etsn",
    // .NET escape hatches
    "add-type",
    "new-object",
];

/// Check if a content string matches any dangerous pattern using the 5-variant check.
fn content_matches_dangerous_pattern(content: &str, pattern: &str) -> bool {
    let lower = pattern.to_lowercase();
    content == lower
        || content == format!("{lower}:*")
        || content == format!("{lower}*")
        || content == format!("{lower} *")
        || (content.starts_with(&format!("{lower} -")) && content.ends_with('*'))
}

/// Check if a Bash permission rule is dangerous for auto mode.
///
/// A rule is dangerous if it would auto-allow commands that execute arbitrary
/// code, bypassing the classifier's safety evaluation.
pub fn is_dangerous_bash_permission(
    tool_name: &str,
    rule_content: Option<&str>,
    is_ant_user: bool,
) -> bool {
    if tool_name != ToolName::Bash.as_str() {
        return false;
    }

    // Tool-level allow with no content restriction → allows ALL commands
    let content = match rule_content {
        None | Some("") => return true,
        Some(c) => c.trim().to_lowercase(),
    };

    if content == "*" {
        return true;
    }

    for pattern in DANGEROUS_BASH_PATTERNS {
        if content_matches_dangerous_pattern(&content, pattern) {
            return true;
        }
    }

    if is_ant_user {
        for pattern in DANGEROUS_BASH_PATTERNS_ANT_ONLY {
            if content_matches_dangerous_pattern(&content, pattern) {
                return true;
            }
        }
    }

    false
}

/// Check if a PowerShell permission rule is dangerous for auto mode.
///
/// Checks both the original pattern and its `.exe` variant (e.g. "npm run"
/// also checks "npm.exe run"). Multi-word patterns add `.exe` after the first
/// word only.
pub fn is_dangerous_powershell_permission(tool_name: &str, rule_content: Option<&str>) -> bool {
    if tool_name != ToolName::PowerShell.as_str() {
        return false;
    }

    let content = match rule_content {
        None | Some("") => return true,
        Some(c) => c.trim().to_lowercase(),
    };

    if content == "*" {
        return true;
    }

    // Combine cross-platform + PS-specific patterns
    let all_patterns: Vec<&str> = CROSS_PLATFORM_CODE_EXEC
        .iter()
        .chain(DANGEROUS_POWERSHELL_PATTERNS.iter())
        .copied()
        .collect();

    for pattern in &all_patterns {
        // Check original pattern
        if content_matches_dangerous_pattern(&content, pattern) {
            return true;
        }

        // Check .exe variant: "npm run" → "npm.exe run"
        let exe_variant = match pattern.find(' ') {
            Some(sp) => format!("{}.exe{}", &pattern[..sp], &pattern[sp..]),
            None => format!("{pattern}.exe"),
        };
        if content_matches_dangerous_pattern(&content, &exe_variant) {
            return true;
        }
    }

    false
}

/// Info about a dangerous permission rule found during setup validation.
#[derive(Debug, Clone)]
pub struct DangerousPermissionInfo {
    pub rule_value: PermissionRuleValue,
    pub source: PermissionRuleSource,
    /// Display string, e.g. `"Bash(*)"` or `"Bash(python:*)"`.
    pub rule_display: String,
    /// Display string for the source, e.g. a file path or `"--allowed-tools"`.
    pub source_display: String,
}

/// Scan `rules` and `cli_allowed_tools` for dangerous classifier-bypass rules.
pub fn find_dangerous_classifier_permissions(
    rules: &[PermissionRule],
    cli_allowed_tools: &[String],
    is_ant_user: bool,
) -> Vec<DangerousPermissionInfo> {
    let mut dangerous = Vec::new();

    for rule in rules {
        if rule.behavior != PermissionBehavior::Allow {
            continue;
        }
        let is_bash_dangerous = is_dangerous_bash_permission(
            &rule.value.tool_pattern,
            rule.value.rule_content.as_deref(),
            is_ant_user,
        );
        let is_ps_dangerous = is_dangerous_powershell_permission(
            &rule.value.tool_pattern,
            rule.value.rule_content.as_deref(),
        );
        // Any `Agent` allow rule bypasses the sub-agent classifier — flag it at
        // startup, mirroring the strip-path predicate (is_dangerous_agent_permission).
        let is_agent_dangerous = rule.value.tool_pattern == coco_types::ToolName::Agent.as_str();
        if is_bash_dangerous || is_ps_dangerous || is_agent_dangerous {
            let display = match &rule.value.rule_content {
                Some(c) => format!("{}({c})", rule.value.tool_pattern),
                None => format!("{}(*)", rule.value.tool_pattern),
            };
            dangerous.push(DangerousPermissionInfo {
                rule_value: rule.value.clone(),
                source: rule.source,
                rule_display: display,
                source_display: permission_source_str(rule.source).to_string(),
            });
        }
    }

    // Check CLI --allowed-tools arguments
    for spec in cli_allowed_tools {
        let (tool_name, rule_content) = parse_tool_spec(spec);
        if is_dangerous_bash_permission(tool_name, rule_content, is_ant_user)
            || is_dangerous_powershell_permission(tool_name, rule_content)
            || tool_name == coco_types::ToolName::Agent.as_str()
        {
            let display =
                rule_content.map_or_else(|| format!("{tool_name}(*)"), |_| spec.to_string());
            dangerous.push(DangerousPermissionInfo {
                rule_value: PermissionRuleValue {
                    tool_pattern: tool_name.to_string(),
                    rule_content: rule_content.map(str::to_string),
                },
                source: PermissionRuleSource::CliArg,
                rule_display: display,
                source_display: "--allowed-tools".to_string(),
            });
        }
    }

    dangerous
}

/// Parse a tool spec like `"Bash(git *)"` into `("Bash", Some("git *"))`.
fn parse_tool_spec(spec: &str) -> (&str, Option<&str>) {
    if let Some(open) = spec.find('(')
        && let Some(close) = spec.rfind(')')
    {
        let tool_name = &spec[..open];
        let content = spec[open + 1..close].trim();
        if content.is_empty() {
            return (tool_name, None);
        }
        return (tool_name, Some(content));
    }
    (spec, None)
}

// ── Default rules ──

/// Generate the default permission rules for a new session.
///
/// Default mode grants read-only tools and denies nothing beyond what
/// mode-based fallthrough handles. The returned rules should be placed
/// in the `Session` source so they can be overridden by persistent rules.
pub fn default_session_rules() -> Vec<PermissionRule> {
    let read_tools = [
        ToolName::Read,
        ToolName::Glob,
        ToolName::Grep,
        ToolName::ToolSearch,
    ];

    read_tools
        .iter()
        .map(|tool| PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: tool.as_str().to_string(),
                rule_content: None,
            },
        })
        .collect()
}

/// Generate default permission rules for a given permission mode.
///
/// Each mode starts with a different baseline of allowed tools:
/// - **Default / Auto**: read-only tools only.
/// - **AcceptEdits**: read-only + file editing tools.
/// - **BypassPermissions / DontAsk**: wildcard allow-all.
/// - **Plan**: read-only + plan-mode tools; all writes denied.
/// - **Bubble**: no session rules (parent agent decides).
pub fn get_default_rules_for_mode(mode: PermissionMode) -> Vec<PermissionRule> {
    match mode {
        PermissionMode::Default | PermissionMode::Auto => default_session_rules(),
        PermissionMode::AcceptEdits => {
            let allowed = [
                ToolName::Read,
                ToolName::Glob,
                ToolName::Grep,
                ToolName::ToolSearch,
                ToolName::Edit,
                ToolName::Write,
                ToolName::NotebookEdit,
            ];
            allowed
                .iter()
                .map(|tool| PermissionRule {
                    source: PermissionRuleSource::Session,
                    behavior: PermissionBehavior::Allow,
                    value: PermissionRuleValue {
                        tool_pattern: tool.as_str().to_string(),
                        rule_content: None,
                    },
                })
                .collect()
        }
        PermissionMode::BypassPermissions | PermissionMode::DontAsk => {
            vec![PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Allow,
                value: PermissionRuleValue {
                    tool_pattern: "*".to_string(),
                    rule_content: None,
                },
            }]
        }
        PermissionMode::Plan => {
            // Tools with is_read_only()=true are allowed in plan mode;
            // whitelisted statically here.
            let read_tools = [
                ToolName::Read,
                ToolName::Glob,
                ToolName::Grep,
                ToolName::ToolSearch,
                ToolName::EnterPlanMode,
                ToolName::ExitPlanMode,
                ToolName::VerifyPlanExecution,
                ToolName::WebFetch,
                ToolName::WebSearch,
                ToolName::AskUserQuestion,
                ToolName::TaskList,
                ToolName::TaskGet,
                ToolName::TaskOutput,
                ToolName::SendUserMessage,
                ToolName::CronList,
                ToolName::Lsp,
                ToolName::ListMcpResources,
                ToolName::ReadMcpResource,
                ToolName::StructuredOutput,
            ];
            read_tools
                .iter()
                .map(|tool| PermissionRule {
                    source: PermissionRuleSource::Session,
                    behavior: PermissionBehavior::Allow,
                    value: PermissionRuleValue {
                        tool_pattern: tool.as_str().to_string(),
                        rule_content: None,
                    },
                })
                .collect()
        }
        PermissionMode::Bubble => Vec::new(),
    }
}

// ── Validation ──

/// A problem found during permission configuration validation.
#[derive(Debug, Clone)]
pub struct PermissionConfigError {
    /// Human-readable description of the issue.
    pub message: String,
    /// Severity: `"error"` blocks startup, `"warning"` is informational.
    pub severity: &'static str,
}

/// Validate a full set of permission rules for internal consistency.
///
/// Checks for:
/// - Conflicting allow + deny rules for the same tool from the same source.
/// - Overly broad rules (e.g. `Bash(*)` in auto mode) detected via
///   `find_dangerous_classifier_permissions`.
/// - Invalid rule strings that failed parsing.
///
/// Returns a list of problems. An empty list means the config is valid.
pub fn validate_permission_configuration(
    rules: &[PermissionRule],
    mode: PermissionMode,
    cli_allowed_tools: &[String],
    is_ant_user: bool,
) -> Vec<PermissionConfigError> {
    let mut errors = Vec::new();

    // Check for same-source allow+deny conflicts
    for (i, a) in rules.iter().enumerate() {
        for b in &rules[i + 1..] {
            if a.source == b.source
                && a.value.tool_pattern == b.value.tool_pattern
                && a.value.rule_content == b.value.rule_content
                && a.behavior != b.behavior
                && matches!(
                    (a.behavior, b.behavior),
                    (PermissionBehavior::Allow, PermissionBehavior::Deny)
                        | (PermissionBehavior::Deny, PermissionBehavior::Allow)
                )
            {
                let display = rule_compiler::rule_value_to_string(&a.value);
                errors.push(PermissionConfigError {
                    message: format!(
                        "conflicting allow+deny rules for '{display}' from source {:?}",
                        a.source
                    ),
                    severity: "error",
                });
            }
        }
    }

    // In auto mode, check for dangerous classifier-bypass rules
    if mode == PermissionMode::Auto {
        let dangerous =
            find_dangerous_classifier_permissions(rules, cli_allowed_tools, is_ant_user);
        for d in &dangerous {
            errors.push(PermissionConfigError {
                message: format!(
                    "dangerous auto-mode rule '{}' from {} bypasses classifier safety checks",
                    d.rule_display, d.source_display,
                ),
                severity: "warning",
            });
        }
    }

    // Validate rule strings parse correctly (detect empty tool_pattern)
    for rule in rules {
        if rule.value.tool_pattern.is_empty() {
            errors.push(PermissionConfigError {
                message: "rule with empty tool pattern".to_string(),
                severity: "error",
            });
        }
    }

    errors
}

/// Resolve the effective permission mode from settings, CLI override, and
/// plan-mode toggle.
///
/// Priority (high → low):
/// 1. Plan mode toggle (user switched to plan mode mid-session)
/// 2. CLI `--permission-mode` flag
/// 3. Settings `permissions.default_mode`
/// 4. `PermissionMode::Default`
pub fn resolve_permission_mode(
    settings_default: Option<PermissionMode>,
    cli_override: Option<PermissionMode>,
    plan_mode_active: bool,
) -> PermissionMode {
    if plan_mode_active {
        return PermissionMode::Plan;
    }
    cli_override
        .or(settings_default)
        .unwrap_or(PermissionMode::Default)
}

fn permission_source_str(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::UserSettings => "userSettings",
        PermissionRuleSource::ProjectSettings => "projectSettings",
        PermissionRuleSource::LocalSettings => "localSettings",
        PermissionRuleSource::FlagSettings => "flagSettings",
        PermissionRuleSource::PolicySettings => "policySettings",
        PermissionRuleSource::CliArg => "cliArg",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
    }
}

#[cfg(test)]
#[path = "setup.test.rs"]
mod tests;
