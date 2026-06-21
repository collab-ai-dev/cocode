//! Core permission evaluation pipeline.
//!
//! Organized as 7 sequential steps. Steps that require external integration
//! (classifier, hooks, sandbox) are deferred to the caller via the returned decision.

use coco_types::MCP_TOOL_PREFIX;
use coco_types::MCP_TOOL_SEPARATOR;
use coco_types::PermissionBehavior;
use coco_types::PermissionDecision;
use coco_types::PermissionDecisionReason;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolPermissionContext;
use serde_json::Value;

use crate::filesystem;
use crate::shell_rules;

// `ToolCheckResult` lives in `coco-types::permission` so the
// `coco_tool_runtime::Tool::check_permissions` trait method can
// reference it without `coco-tool-runtime` depending on
// `coco-permissions`. Re-exported below for legacy import sites.
pub use coco_types::ToolCheckResult;

/// Callback for tool-level permission checks.
///
/// Each tool (Bash, Write, PowerShell) implements content-specific checks
/// that the rule pipeline can't express (subcommand parsing, path safety, etc.).
///
/// The evaluate pipeline calls this after deny rules (step 1) and before
/// allow rules (step 2). The tool can return `Passthrough` to defer to rules.
pub type ToolCheckFn = dyn Fn(&ToolId, &Value, &ToolPermissionContext) -> ToolCheckResult;

/// Per-call facts supplied by the tool runtime to the rule evaluator.
///
/// These are facts about the already-validated tool input, not policy.
/// Keeping them explicit avoids making `coco-permissions` depend on
/// shell/file parsers owned by tool crates.
#[derive(Debug, Clone, Default)]
pub struct PermissionEvaluationOptions {
    /// The concrete tool/input pair is read-only even though the tool name
    /// itself may not be statically read-only, e.g. `Bash(ls | head)`.
    pub dynamic_read_only: bool,
    /// The caller has determined this is a Bash command that WILL be sandboxed
    /// AND `autoAllowBashIfSandboxed` is enabled. When true, a tool-wide Bash
    /// ASK rule is skipped and (absent a deny/content-ask/allow match) the
    /// command auto-allows. Computed in `app/query` (which holds
    /// `ctx.sandbox_state`) so `coco-permissions` keeps no `exec/sandbox` dependency.
    pub sandbox_auto_allow_bash: bool,
    /// Effective Bash cwd for cwd-sensitive permission suggestions.
    pub shell_cwd: Option<String>,
}

/// Permission evaluator. Implements the multi-step evaluation pipeline.
pub struct PermissionEvaluator;

impl PermissionEvaluator {
    /// Evaluate permissions for a tool call.
    ///
    /// Pipeline:
    /// 1. Deny rules — deny always wins, regardless of priority
    ///    1b. Tool-level permission check (via callback)
    /// 2. Allow rules — priority-sorted by source
    /// 3. Content-specific allow — Bash/PowerShell exact/prefix/wildcard
    /// 4. Ask rules — tool-wide ask requires user confirmation
    /// 5. Content-specific ask — Bash(pattern) ask rules
    /// 6. Path safety checks — dangerous files, traversal
    /// 7. MCP server-level rules
    /// 8. Mode-based fallthrough
    pub fn evaluate(
        tool_id: &ToolId,
        input: &Value,
        context: &ToolPermissionContext,
    ) -> PermissionDecision {
        Self::evaluate_with_tool_check(tool_id, input, context, None)
    }

    /// Evaluate with an optional tool-level permission callback.
    ///
    /// Bash uses this for subcommand analysis, Write uses for path checks, etc.
    pub fn evaluate_with_tool_check(
        tool_id: &ToolId,
        input: &Value,
        context: &ToolPermissionContext,
        tool_check: Option<&ToolCheckFn>,
    ) -> PermissionDecision {
        Self::evaluate_with_tool_check_and_options(
            tool_id,
            input,
            context,
            tool_check,
            PermissionEvaluationOptions::default(),
        )
    }

    /// Evaluate with an optional tool-level permission callback and
    /// per-call runtime facts.
    pub fn evaluate_with_tool_check_and_options(
        tool_id: &ToolId,
        input: &Value,
        context: &ToolPermissionContext,
        tool_check: Option<&ToolCheckFn>,
        options: PermissionEvaluationOptions,
    ) -> PermissionDecision {
        let decision = Self::evaluate_inner(tool_id, input, context, tool_check, options);
        // dontAsk converts ANY remaining 'ask' into 'deny' as the final step,
        // so early-return asks (tool-wide ask, content ask, path-safety ask,
        // MCP server ask) are denied too — not only the mode-fallthrough ask.
        if context.mode == PermissionMode::DontAsk
            && matches!(decision, PermissionDecision::Ask { .. })
        {
            return PermissionDecision::Deny {
                message: format!("{tool_id} denied: permission mode does not allow prompting"),
                reason: PermissionDecisionReason::Mode {
                    mode: PermissionMode::DontAsk,
                },
            };
        }
        decision
    }

    fn evaluate_inner(
        tool_id: &ToolId,
        input: &Value,
        context: &ToolPermissionContext,
        tool_check: Option<&ToolCheckFn>,
        options: PermissionEvaluationOptions,
    ) -> PermissionDecision {
        let tool_str = tool_id.to_string();

        tracing::trace!(
            tool_name = %tool_str,
            mode = ?context.mode,
            deny_rule_sources = context.deny_rules.len(),
            allow_rule_sources = context.allow_rules.len(),
            ask_rule_sources = context.ask_rules.len(),
            has_tool_check = tool_check.is_some(),
            dynamic_read_only = options.dynamic_read_only,
            "permission_eval: enter",
        );

        // Step 1: Deny rules (deny always wins). Only tool-WIDE denies and
        // CONTENT-MATCHED shell/file denies fire here; content-bearing denies
        // for Agent/WebFetch/Read/Grep/Glob defer to the tool's step-1b
        // check_permissions (the central over-deny fail-closed-bug fix).
        for rules in context.deny_rules.values() {
            for rule in rules {
                if !central_rule_applies(
                    rule,
                    &tool_str,
                    input,
                    context,
                    shell_rules::RuleMatchPolicy::DenyOrAsk,
                ) {
                    continue;
                }
                tracing::debug!(
                    tool_name = %tool_str,
                    permission_decision = "deny",
                    rule_pattern = %rule.value.tool_pattern,
                    rule_content = ?rule.value.rule_content,
                    "permission_eval: deny rule matched",
                );
                return PermissionDecision::Deny {
                    message: format!("denied by rule: {}", rule.value.tool_pattern),
                    reason: PermissionDecisionReason::Rule { rule: rule.clone() },
                };
            }
        }

        // Step 1b: Tool-level permission check
        // Bash checks subcommands, PowerShell checks cmdlets, Write checks paths.
        if let Some(check_fn) = tool_check {
            match check_fn(tool_id, input, context) {
                ToolCheckResult::Deny { message } => {
                    tracing::debug!(
                        tool_name = %tool_str,
                        permission_decision = "deny",
                        "permission_eval: tool.check_permissions returned deny",
                    );
                    return PermissionDecision::Deny {
                        message,
                        reason: PermissionDecisionReason::Mode { mode: context.mode },
                    };
                }
                ToolCheckResult::Allow {
                    updated_input,
                    feedback,
                } => {
                    tracing::debug!(
                        tool_name = %tool_str,
                        permission_decision = "allow",
                        rewritten_input = updated_input.is_some(),
                        "permission_eval: tool.check_permissions returned allow",
                    );
                    return PermissionDecision::Allow {
                        updated_input,
                        feedback,
                    };
                }
                ToolCheckResult::Ask {
                    message,
                    suggestions,
                    choices,
                    detail,
                } => {
                    tracing::debug!(
                        tool_name = %tool_str,
                        permission_decision = "ask",
                        suggestion_count = suggestions.len(),
                        "permission_eval: tool.check_permissions returned ask",
                    );
                    return PermissionDecision::Ask {
                        message,
                        suggestions,
                        choices,
                        detail,
                    };
                }
                ToolCheckResult::Passthrough => {
                    tracing::trace!(
                        tool_name = %tool_str,
                        "permission_eval: tool.check_permissions passthrough",
                    );
                    // Tool has no opinion — continue pipeline
                }
            }
        }

        // Step 2/3: Allow rules (sorted by source priority). Tool-WIDE allows
        // and CONTENT-MATCHED shell/file allows fire here; content-bearing
        // allows for Agent/WebFetch/Read defer to the tool's step-1b check.
        // NOTE: a content-bearing shell allow with no `command` field no longer
        // broadly allows (central_rule_applies returns false) — allow rules key
        // on content, and a missing command never matches.
        for source in RULE_PRIORITY_ORDER {
            if let Some(rules) = context.allow_rules.get(source) {
                for rule in rules {
                    if !central_rule_applies(
                        rule,
                        &tool_str,
                        input,
                        context,
                        shell_rules::RuleMatchPolicy::Allow,
                    ) {
                        continue;
                    }
                    tracing::debug!(
                        tool_name = %tool_str,
                        permission_decision = "allow",
                        rule_source = ?source,
                        rule_pattern = %rule.value.tool_pattern,
                        rule_content = ?rule.value.rule_content,
                        "permission_eval: allow rule matched",
                    );
                    return PermissionDecision::Allow {
                        updated_input: None,
                        feedback: None,
                    };
                }
            }
        }

        // When a tool-wide Bash ask rule is skipped because the command will be
        // sandboxed (sandbox auto-allow), the evaluator must auto-allow on
        // fall-through rather than re-prompt via mode_fallthrough.
        let mut sandbox_skip_allow = false;

        // Step 4: Ask rules — tool-wide ask
        if let Some(ask_rule) = get_tool_wide_rule(context, &tool_str, PermissionBehavior::Ask) {
            // Sandbox auto-allow: skip a tool-wide Bash ask rule when the command
            // will be sandboxed and autoAllowBashIfSandboxed is on.
            // Content-specific Bash ask rules are STILL honored below
            // (per-command asks are preserved).
            let skip_tool_wide_ask =
                options.sandbox_auto_allow_bash && is_shell_tool(&ask_rule.value.tool_pattern);

            // If this is a shell tool, check if there are content-specific rules first
            if is_shell_tool(&ask_rule.value.tool_pattern)
                && let Some(command) = extract_shell_command(input)
            {
                // Step 5: Content-specific ask rules
                for rules in context.ask_rules.values() {
                    for rule in rules {
                        if matches_tool_pattern(&rule.value.tool_pattern, &tool_str)
                            && let Some(content) = &rule.value.rule_content
                            && shell_rules::match_bash_rule(
                                content,
                                &command,
                                shell_rules::RuleMatchPolicy::DenyOrAsk,
                                shell_case_for(&rule.value.tool_pattern),
                            )
                        {
                            tracing::debug!(
                                tool_name = %tool_str,
                                permission_decision = "ask",
                                rule_pattern = %rule.value.tool_pattern,
                                rule_content = %content,
                                "permission_eval: shell ask rule matched",
                            );
                            return PermissionDecision::Ask {
                                message: format!("ask rule matched: {tool_str}({content})"),
                                suggestions: vec![],
                                choices: None,
                                detail: None,
                            };
                        }
                    }
                }
            }

            if skip_tool_wide_ask {
                tracing::debug!(
                    tool_name = %tool_str,
                    rule_pattern = %ask_rule.value.tool_pattern,
                    "permission_eval: tool-wide Bash ask rule skipped (sandbox auto-allow)",
                );
                // Defer the auto-allow to after the remaining ask/path/MCP steps
                // so a content-specific ask or MCP rule can still pre-empt it.
                sandbox_skip_allow = true;
            } else {
                tracing::debug!(
                    tool_name = %tool_str,
                    permission_decision = "ask",
                    rule_pattern = %ask_rule.value.tool_pattern,
                    "permission_eval: tool-wide ask rule matched",
                );
                return PermissionDecision::Ask {
                    message: format!("tool-wide ask rule for {tool_str}"),
                    suggestions: vec![],
                    choices: None,
                    detail: None,
                };
            }
        }

        for rules in context.ask_rules.values() {
            for rule in rules {
                if is_file_rule_for_tool(&rule.value.tool_pattern, &tool_str)
                    && rule.value.rule_content.is_some()
                    && file_rule_matches_input(rule, &tool_str, input, context)
                {
                    tracing::debug!(
                        tool_name = %tool_str,
                        permission_decision = "ask",
                        rule_pattern = %rule.value.tool_pattern,
                        rule_content = ?rule.value.rule_content,
                        "permission_eval: file ask rule matched",
                    );
                    return PermissionDecision::Ask {
                        message: format!(
                            "ask rule matched: {tool_str}({})",
                            rule.value.rule_content.as_deref().unwrap_or("")
                        ),
                        suggestions: vec![],
                        choices: None,
                        detail: None,
                    };
                }
            }
        }

        // Step 6: Path safety checks for file-modifying tools
        if is_file_modifying_tool(&tool_str)
            && let Some(path) = extract_file_path(input)
        {
            let safety = filesystem::check_path_safety_for_auto_edit(&path);
            if let filesystem::PathSafetyResult::Blocked {
                message,
                classifier_approvable: _,
            } = safety
            {
                tracing::debug!(
                    tool_name = %tool_str,
                    permission_decision = "ask",
                    path = %path,
                    "permission_eval: path safety check blocked",
                );
                return PermissionDecision::Ask {
                    message,
                    suggestions: vec![],
                    choices: None,
                    detail: None,
                };
            }
        }

        // Step 7: MCP server-level rules
        if let ToolId::Mcp { server, .. } = tool_id {
            // Check server-level allow: "mcp__server" matches "mcp__server__tool".
            // Server-level rules are tool-wide (no content); content-bearing
            // tool-level MCP rules go through central_rule_applies so they stay
            // scoped (no unconditional fire).
            let server_pattern = format!("{MCP_TOOL_PREFIX}{server}");
            for rules in context.allow_rules.values() {
                for rule in rules {
                    let server_level = rule.value.tool_pattern == server_pattern
                        && rule.value.rule_content.is_none();
                    if server_level
                        || central_rule_applies(
                            rule,
                            &tool_str,
                            input,
                            context,
                            shell_rules::RuleMatchPolicy::Allow,
                        )
                    {
                        tracing::debug!(
                            tool_name = %tool_str,
                            permission_decision = "allow",
                            mcp_server = %server,
                            rule_pattern = %rule.value.tool_pattern,
                            "permission_eval: MCP server-level allow rule matched",
                        );
                        return PermissionDecision::Allow {
                            updated_input: None,
                            feedback: None,
                        };
                    }
                }
            }

            // Check server-level ask (tool-wide by definition)
            for rules in context.ask_rules.values() {
                for rule in rules {
                    if rule.value.tool_pattern == server_pattern
                        && rule.value.rule_content.is_none()
                    {
                        tracing::debug!(
                            tool_name = %tool_str,
                            permission_decision = "ask",
                            mcp_server = %server,
                            "permission_eval: MCP server-level ask rule matched",
                        );
                        return PermissionDecision::Ask {
                            message: format!("MCP server {server} requires approval"),
                            suggestions: vec![],
                            choices: None,
                            detail: None,
                        };
                    }
                }
            }
        }

        // Sandbox auto-allow: a tool-wide Bash ask rule was skipped and no deny /
        // content-ask / allow / MCP rule matched, so the sandboxed command
        // auto-allows here instead of falling to mode_fallthrough (which would
        // re-prompt a non-read-only Bash).
        if sandbox_skip_allow {
            tracing::debug!(
                tool_name = %tool_str,
                permission_decision = "allow",
                "permission_eval: sandbox auto-allow (autoAllowBashIfSandboxed)",
            );
            return PermissionDecision::Allow {
                updated_input: None,
                feedback: None,
            };
        }

        // Step 8: Mode-based fallthrough
        let decision = mode_fallthrough(context, &tool_str, input, &options);
        tracing::debug!(
            tool_name = %tool_str,
            permission_decision = decision_label(&decision),
            mode = ?context.mode,
            dynamic_read_only = options.dynamic_read_only,
            "permission_eval: fell through to mode-based decision",
        );
        decision
    }
}

/// Short tag for a `PermissionDecision` suitable for the
/// `permission_decision` tracing field.
fn decision_label(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::Allow { .. } => "allow",
        PermissionDecision::Deny { .. } => "deny",
        PermissionDecision::Ask { .. } => "ask",
        PermissionDecision::Abort { .. } => "abort",
    }
}

// ── Rule helpers ──

/// Collect all rules of a given behavior from all sources (flattened).
pub fn get_all_rules(
    context: &ToolPermissionContext,
    behavior: PermissionBehavior,
) -> Vec<&PermissionRule> {
    let map = match behavior {
        PermissionBehavior::Allow => &context.allow_rules,
        PermissionBehavior::Deny => &context.deny_rules,
        PermissionBehavior::Ask => &context.ask_rules,
    };
    map.values().flatten().collect()
}

/// Find a tool-wide rule (no content constraint) for a specific tool.
pub fn get_tool_wide_rule(
    context: &ToolPermissionContext,
    tool_str: &str,
    behavior: PermissionBehavior,
) -> Option<PermissionRule> {
    let map = match behavior {
        PermissionBehavior::Allow => &context.allow_rules,
        PermissionBehavior::Deny => &context.deny_rules,
        PermissionBehavior::Ask => &context.ask_rules,
    };

    for rules in map.values() {
        for rule in rules {
            if matches_tool_pattern(&rule.value.tool_pattern, tool_str)
                && rule.value.rule_content.is_none()
            {
                return Some(rule.clone());
            }
        }
    }
    None
}

/// Get all content-specific rules for a tool (e.g. all Bash(pattern) rules).
pub fn get_content_rules_for_tool<'a>(
    context: &'a ToolPermissionContext,
    tool_str: &str,
    behavior: PermissionBehavior,
) -> Vec<&'a PermissionRule> {
    let map = match behavior {
        PermissionBehavior::Allow => &context.allow_rules,
        PermissionBehavior::Deny => &context.deny_rules,
        PermissionBehavior::Ask => &context.ask_rules,
    };

    map.values()
        .flatten()
        .filter(|r| {
            matches_tool_pattern(&r.value.tool_pattern, tool_str) && r.value.rule_content.is_some()
        })
        .collect()
}

// ── Priority ──

/// Rule source priority order (most specific first).
const RULE_PRIORITY_ORDER: &[PermissionRuleSource] = &[
    PermissionRuleSource::Session,
    PermissionRuleSource::Command,
    PermissionRuleSource::CliArg,
    PermissionRuleSource::FlagSettings,
    PermissionRuleSource::LocalSettings,
    PermissionRuleSource::ProjectSettings,
    PermissionRuleSource::UserSettings,
    PermissionRuleSource::PolicySettings,
];

// ── Mode fallthrough ──

/// Mode-based fallthrough when no rules matched.
///
/// - `bypassPermissions` → auto-allow everything
/// - `dontAsk` → deny
/// - `plan` with bypass_available → auto-allow
/// - `plan` without bypass_available → ask
/// - `acceptEdits` → auto-allow read-only + file edits; ask for rest
/// - `default`, `auto` → auto-allow read-only; ask for rest
/// - `bubble` → ask and delegate to the parent context
fn mode_fallthrough(
    context: &ToolPermissionContext,
    tool_str: &str,
    input: &Value,
    options: &PermissionEvaluationOptions,
) -> PermissionDecision {
    match context.mode {
        PermissionMode::BypassPermissions => PermissionDecision::Allow {
            updated_input: None,
            feedback: None,
        },
        // dontAsk converts every remaining 'ask' into 'deny'.
        // "Don't ask me — just deny anything that would prompt."
        PermissionMode::DontAsk => PermissionDecision::Deny {
            message: format!("{tool_str} denied: permission mode does not allow prompting"),
            reason: PermissionDecisionReason::Mode {
                mode: PermissionMode::DontAsk,
            },
        },
        // Plan mode auto-allows if user originally had bypass mode;
        // otherwise falls through to ask (for classifier or user prompt).
        // Read-only tools are always safe in plan mode. Writes/edits to the
        // session's own plan file also auto-allow, but that carve-out lives in
        // the file-tool `check_permissions` layer (step 1b →
        // `is_editable_internal_path` / `is_session_plan_file`), which runs
        // before this fallthrough — so a plan-file write never reaches here.
        PermissionMode::Plan => {
            if context.bypass_available || is_read_only_tool(tool_str) || options.dynamic_read_only
            {
                PermissionDecision::Allow {
                    updated_input: None,
                    feedback: None,
                }
            } else {
                PermissionDecision::Ask {
                    message: format!("plan mode: approve {tool_str}?"),
                    suggestions: shell_ask_suggestions(
                        tool_str,
                        input,
                        options.shell_cwd.as_deref(),
                    ),
                    choices: None,
                    detail: None,
                }
            }
        }
        // acceptEdits auto-allows read-only tools AND file-modifying
        // tools (Write, Edit, NotebookEdit). Dangerous paths are already
        // caught by step 6 (path safety), so only safe edits reach here.
        PermissionMode::AcceptEdits => {
            if is_read_only_tool(tool_str)
                || options.dynamic_read_only
                || is_file_modifying_tool(tool_str)
            {
                PermissionDecision::Allow {
                    updated_input: None,
                    feedback: None,
                }
            } else {
                PermissionDecision::Ask {
                    message: format!("approve {tool_str}?"),
                    suggestions: shell_ask_suggestions(
                        tool_str,
                        input,
                        options.shell_cwd.as_deref(),
                    ),
                    choices: None,
                    detail: None,
                }
            }
        }
        // Safe read-only tools never need an approval prompt in ordinary
        // interactive modes. Keep Bubble out of this fast path so the parent
        // permission context remains authoritative.
        PermissionMode::Default | PermissionMode::Auto
            if is_read_only_tool(tool_str) || options.dynamic_read_only =>
        {
            PermissionDecision::Allow {
                updated_input: None,
                feedback: None,
            }
        }
        PermissionMode::Default | PermissionMode::Auto | PermissionMode::Bubble => {
            PermissionDecision::Ask {
                message: format!("approve {tool_str}?"),
                suggestions: shell_ask_suggestions(tool_str, input, options.shell_cwd.as_deref()),
                choices: None,
                detail: None,
            }
        }
    }
}

/// "Always allow `<prefix>:*`" suggestions for a shell command that fell
/// through to a mode-based approval prompt with no matching allow rule.
///
/// Non-shell tools and inputs without a `command` string yield no suggestion.
///
/// The dangerous-gate asks raised by Bash's own `check_permissions` (rm,
/// git-escape, process substitution, out-of-tree writes, …) keep their empty
/// suggestions — declining to suggest saving a potentially dangerous command.
fn shell_ask_suggestions(
    tool_str: &str,
    input: &Value,
    shell_cwd: Option<&str>,
) -> Vec<coco_types::PermissionUpdate> {
    if !is_shell_tool(tool_str) {
        return Vec::new();
    }
    match extract_shell_command(input) {
        Some(command) => match shell_cwd {
            Some(cwd) => shell_rules::bash_permission_suggestions_in_cwd(tool_str, &command, cwd),
            None => shell_rules::bash_permission_suggestions(tool_str, &command),
        },
        None => Vec::new(),
    }
}

/// Extract the file-path argument from a file-modifying tool's input.
///
/// Write + Edit use `file_path`; NotebookEdit uses `notebook_path`.
pub(crate) fn extract_file_modifying_path(tool_str: &str, input: &Value) -> Option<String> {
    let key = if tool_str == ToolName::NotebookEdit.as_str() {
        "notebook_path"
    } else {
        "file_path"
    };
    input.get(key).and_then(|v| v.as_str()).map(String::from)
}

// ── Predicates ──

/// Tools that are always safe to auto-allow (no side effects).
///
/// These tools skip the classifier in auto mode and are auto-allowed
/// in acceptEdits/plan modes.
fn is_read_only_tool(tool_name: &str) -> bool {
    const READ_ONLY_TOOLS: &[&str] = &[
        // File I/O (read-only)
        ToolName::Read.as_str(),
        ToolName::Glob.as_str(),
        ToolName::Grep.as_str(),
        ToolName::Lsp.as_str(),
        ToolName::ToolSearch.as_str(),
        // MCP read-only
        ToolName::ListMcpResources.as_str(),
        ToolName::ReadMcpResource.as_str(),
        // Task management (metadata only)
        ToolName::TodoWrite.as_str(),
        ToolName::TaskCreate.as_str(),
        ToolName::TaskGet.as_str(),
        ToolName::TaskUpdate.as_str(),
        ToolName::TaskList.as_str(),
        ToolName::TaskStop.as_str(),
        ToolName::TaskOutput.as_str(),
        // Plan mode / UI (control flow, no execution)
        ToolName::AskUserQuestion.as_str(),
        ToolName::EnterPlanMode.as_str(),
        ToolName::ExitPlanMode.as_str(),
        ToolName::VerifyPlanExecution.as_str(),
        // Swarm coordination (internal state, no external effects)
        ToolName::TeamCreate.as_str(),
        ToolName::TeamDelete.as_str(),
        ToolName::SendMessage.as_str(),
        // Scheduling read-only
        ToolName::CronList.as_str(),
        // Misc safe
        ToolName::Sleep.as_str(),
        ToolName::SendUserMessage.as_str(),
    ];
    READ_ONLY_TOOLS.contains(&tool_name)
}

/// Whether a tool pattern targets a shell tool (Bash/PowerShell).
fn is_shell_tool(tool_pattern: &str) -> bool {
    tool_pattern == ToolName::Bash.as_str() || tool_pattern == ToolName::PowerShell.as_str()
}

/// Case sensitivity for a shell tool's content matching. PowerShell is
/// case-insensitive; Bash is case-sensitive.
fn shell_case_for(tool_pattern: &str) -> shell_rules::ShellCase {
    if tool_pattern == ToolName::PowerShell.as_str() {
        shell_rules::ShellCase::Insensitive
    } else {
        shell_rules::ShellCase::Sensitive
    }
}

/// Whether a tool modifies files (Write, Edit, NotebookEdit).
pub(crate) fn is_file_modifying_tool(tool_name: &str) -> bool {
    tool_name == ToolName::Write.as_str()
        || tool_name == ToolName::Edit.as_str()
        || tool_name == ToolName::NotebookEdit.as_str()
        || tool_name == ToolName::ApplyPatch.as_str()
}

fn is_file_rule_for_tool(rule_tool_pattern: &str, tool_name: &str) -> bool {
    is_file_modifying_tool(tool_name)
        && (rule_tool_pattern == ToolName::Edit.as_str() || rule_tool_pattern == tool_name)
}

fn file_rule_matches_input(
    rule: &PermissionRule,
    tool_name: &str,
    input: &Value,
    context: &ToolPermissionContext,
) -> bool {
    let Some(path) = extract_file_modifying_path(tool_name, input) else {
        return false;
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let paths_to_check = filesystem::get_paths_for_permission_check(&path, &cwd);
    let match_context = crate::file_rules::FileRuleMatchContext::new(&cwd)
        .with_source_roots(&context.permission_rule_source_roots);
    crate::file_rules::file_rule_matches_paths(
        rule,
        &paths_to_check,
        crate::file_rules::FileRuleToolType::Edit,
        &match_context,
    )
}

fn extract_shell_command(input: &Value) -> Option<String> {
    input
        .get("command")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn extract_file_path(input: &Value) -> Option<String> {
    input
        .get("file_path")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Simple tool pattern matching (exact, prefix-wildcard, or MCP pattern).
fn matches_tool_pattern(pattern: &str, tool: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool.starts_with(prefix);
    }
    // Handle Bash(command) pattern: "Bash" matches tool "Bash"
    if let Some(paren) = pattern.find('(') {
        let base = &pattern[..paren];
        return tool == base;
    }
    // MCP server-level: "mcp__server" matches "mcp__server__tool"
    if let Some(pattern_rest) = pattern.strip_prefix(MCP_TOOL_PREFIX)
        && let Some(tool_rest) = tool.strip_prefix(MCP_TOOL_PREFIX)
    {
        // pattern_rest = "server" (no separator = server-level rule)
        // tool_rest = "server__tool"
        if !pattern_rest.contains(MCP_TOOL_SEPARATOR) {
            // Server-level pattern: extract server from tool
            let tool_server = tool_rest
                .split_once(MCP_TOOL_SEPARATOR)
                .map(|(s, _)| s)
                .unwrap_or(tool_rest);
            return pattern_rest == tool_server;
        }
    }
    pattern == tool
}

/// Whether `rule` applies to this tool call at the CENTRAL evaluation step
/// (deny step-1 / allow step-2/3 / MCP step-7). Handles the tool-WIDE case
/// plus content-scoping for shell + file-modifying tools.
///
/// Returns `false` (defer to the tool's step-1b `check_permissions`) when the
/// rule carries content AND the tool is neither a shell tool nor a
/// file-modifying tool — e.g. `Agent(Explore)`, `WebFetch(domain:bad.com)`,
/// `Read(/secret/**)`, `Grep(x)`. Those content rules are scoped by the tool
/// itself; firing them centrally over-denies / over-allows (the fail-closed
/// bug this carve-out fixes).
fn central_rule_applies(
    rule: &PermissionRule,
    tool_str: &str,
    input: &Value,
    context: &ToolPermissionContext,
    policy: shell_rules::RuleMatchPolicy,
) -> bool {
    if !matches_tool_pattern(&rule.value.tool_pattern, tool_str) {
        return false;
    }
    // Tool-wide rule (no content) always applies centrally.
    let Some(content) = rule.value.rule_content.as_deref() else {
        return true;
    };
    // Shell content rule (Bash/PowerShell): apply only if the command matches
    // the pattern under the caller's posture (deny/ask strip env+wrappers and
    // re-split; allow keeps the compound guard). A missing `command` field
    // cannot be scoped → do not match centrally.
    if is_shell_tool(&rule.value.tool_pattern) {
        return match extract_shell_command(input) {
            Some(command) => shell_rules::match_bash_rule(
                content,
                &command,
                policy,
                shell_case_for(&rule.value.tool_pattern),
            ),
            None => false,
        };
    }
    // File content rule (Write/Edit/NotebookEdit/ApplyPatch): apply only if the
    // path matches the rule's glob; otherwise defer.
    if is_file_rule_for_tool(&rule.value.tool_pattern, tool_str) {
        return file_rule_matches_input(rule, tool_str, input, context);
    }
    // Any OTHER tool with a content-bearing rule (Agent/WebFetch/Read/Grep/Glob)
    // NEVER matches centrally — defer to the tool's own scoped check_permissions.
    false
}

#[cfg(test)]
#[path = "evaluate.test.rs"]
mod tests;
