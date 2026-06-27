//! Shadowed (unreachable) rule detection.
//!
//! Detects allow rules that are shadowed by tool-wide ask or deny rules,
//! making them unreachable. Provides fix suggestions.

use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::ToolName;
use coco_types::ToolPermissionContext;

/// Type of shadowing that makes a rule unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowType {
    /// Shadowed by an ask rule (will always prompt, less severe).
    Ask,
    /// Shadowed by a deny rule (completely blocked, more severe).
    Deny,
}

/// An unreachable permission rule with explanation.
#[derive(Debug, Clone)]
pub struct UnreachableRule {
    /// The unreachable allow rule.
    pub rule: PermissionRule,
    /// Human-readable explanation.
    pub reason: String,
    /// The rule that shadows it.
    pub shadowed_by: PermissionRule,
    /// Type of shadowing.
    pub shadow_type: ShadowType,
    /// Suggested fix.
    pub fix: String,
}

/// Options for detecting unreachable rules.
#[derive(Debug, Clone)]
pub struct DetectUnreachableRulesOptions {
    /// Whether sandbox auto-allow is enabled for Bash commands.
    /// When true, tool-wide Bash ask rules from personal settings don't block
    /// specific Bash allow rules because sandboxed commands are auto-allowed.
    pub sandbox_auto_allow_enabled: bool,
}

/// Check if a permission rule source is shared (visible to other users).
///
/// Shared: projectSettings, policySettings, command.
/// Personal: userSettings, localSettings, cliArg, session, flagSettings.
fn is_shared_setting_source(source: PermissionRuleSource) -> bool {
    matches!(
        source,
        PermissionRuleSource::ProjectSettings
            | PermissionRuleSource::PolicySettings
            | PermissionRuleSource::Command
    )
}

/// Display name for a rule source.
fn format_source(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::UserSettings => "user settings",
        PermissionRuleSource::ProjectSettings => "project settings",
        PermissionRuleSource::LocalSettings => "local settings",
        PermissionRuleSource::FlagSettings => "flag settings",
        PermissionRuleSource::PolicySettings => "policy settings",
        PermissionRuleSource::CliArg => "CLI argument",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
    }
}

/// Collect all rules of a given behavior from the context.
fn collect_rules(
    context: &ToolPermissionContext,
    behavior: PermissionBehavior,
) -> Vec<PermissionRule> {
    let source_map = match behavior {
        PermissionBehavior::Allow => &context.allow_rules,
        PermissionBehavior::Deny => &context.deny_rules,
        PermissionBehavior::Ask => &context.ask_rules,
    };
    source_map.values().flatten().cloned().collect()
}

/// Check if an allow rule is shadowed by a tool-wide deny rule.
///
/// An allow rule is unreachable when there's a tool-wide deny rule for the same tool.
/// Deny is checked first in the evaluation pipeline, so the allow never fires.
fn is_shadowed_by_deny(
    allow_rule: &PermissionRule,
    deny_rules: &[PermissionRule],
) -> Option<PermissionRule> {
    let tool_name = &allow_rule.value.tool_pattern;

    // Only content-specific allow rules can be shadowed
    allow_rule.value.rule_content.as_ref()?;

    deny_rules
        .iter()
        .find(|deny| deny.value.tool_pattern == *tool_name && deny.value.rule_content.is_none())
        .cloned()
}

/// Check if an allow rule is shadowed by a tool-wide ask rule.
///
/// An allow rule is unreachable when there's a tool-wide ask rule for the same tool,
/// because the user will always be prompted first.
///
/// Exception: Bash with sandbox auto-allow from personal settings.
fn is_shadowed_by_ask(
    allow_rule: &PermissionRule,
    ask_rules: &[PermissionRule],
    options: &DetectUnreachableRulesOptions,
) -> Option<PermissionRule> {
    let tool_name = &allow_rule.value.tool_pattern;

    // Only content-specific allow rules can be shadowed
    allow_rule.value.rule_content.as_ref()?;

    let shadowing = ask_rules
        .iter()
        .find(|ask| ask.value.tool_pattern == *tool_name && ask.value.rule_content.is_none())?;

    // Sandbox exception: Bash ask rules from personal settings don't shadow
    // when sandbox auto-allow is enabled (sandboxed commands are auto-allowed).
    if *tool_name == ToolName::Bash.as_str()
        && options.sandbox_auto_allow_enabled
        && !is_shared_setting_source(shadowing.source)
    {
        return None;
    }

    Some(shadowing.clone())
}

/// Detect all unreachable permission rules in the given context.
///
/// Currently detects:
/// - Allow rules shadowed by tool-wide deny rules (completely blocked)
/// - Allow rules shadowed by tool-wide ask rules (will always prompt)
pub fn detect_unreachable_rules(
    context: &ToolPermissionContext,
    options: &DetectUnreachableRulesOptions,
) -> Vec<UnreachableRule> {
    let allow_rules = collect_rules(context, PermissionBehavior::Allow);
    let ask_rules = collect_rules(context, PermissionBehavior::Ask);
    let deny_rules = collect_rules(context, PermissionBehavior::Deny);

    let mut unreachable = Vec::new();

    for allow_rule in &allow_rules {
        // Check deny shadowing first (more severe)
        if let Some(shadowing) = is_shadowed_by_deny(allow_rule, &deny_rules) {
            let shadow_source = format_source(shadowing.source);
            let tool_name = &shadowing.value.tool_pattern;
            let allow_source = format_source(allow_rule.source);
            unreachable.push(UnreachableRule {
                rule: allow_rule.clone(),
                reason: format!("Blocked by \"{tool_name}\" deny rule (from {shadow_source})"),
                shadowed_by: shadowing.clone(),
                shadow_type: ShadowType::Deny,
                fix: format!(
                    "Remove the \"{tool_name}\" deny rule from {shadow_source}, \
                     or remove the specific allow rule from {allow_source}"
                ),
            });
            continue; // Don't also report ask-shadowing if deny-shadowed
        }

        // Check ask shadowing
        if let Some(shadowing) = is_shadowed_by_ask(allow_rule, &ask_rules, options) {
            let shadow_source = format_source(shadowing.source);
            let tool_name = &shadowing.value.tool_pattern;
            let allow_source = format_source(allow_rule.source);
            unreachable.push(UnreachableRule {
                rule: allow_rule.clone(),
                reason: format!("Shadowed by \"{tool_name}\" ask rule (from {shadow_source})"),
                shadowed_by: shadowing.clone(),
                shadow_type: ShadowType::Ask,
                fix: format!(
                    "Remove the \"{tool_name}\" ask rule from {shadow_source}, \
                     or remove the specific allow rule from {allow_source}"
                ),
            });
        }
    }

    unreachable
}

#[cfg(test)]
#[path = "shadowed_rules.test.rs"]
mod tests;
