//! Dangerous rule stripping for auto-mode.
//!
//! When entering auto-mode, certain user-configured allow rules that could
//! bypass the classifier's safety checks are temporarily removed and stashed.
//! They are restored when exiting auto-mode.

use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRulesBySource;
use coco_types::ToolName;
use coco_types::ToolPermissionContext;

use crate::setup::is_dangerous_bash_permission;
use crate::setup::is_dangerous_powershell_permission;

/// Filter dangerous classifier-bypassing allow rules out of a raw
/// `allow_rules` map (python:*, node:*, eval, ssh, curl, git, sudo, any Agent,
/// …). Dangerous rules from non-restorable sources (Flag/Policy/Command) are
/// still REMOVED (so they cannot bypass the classifier) but are NOT returned
/// for stashing — they are never restored.
///
/// Returns `None` when nothing was stripped (caller leaves the stash untouched).
pub fn strip_dangerous_allow_rules(
    allow_rules: &mut PermissionRulesBySource,
    is_ant_user: bool,
) -> Option<PermissionRulesBySource> {
    let mut stripped = PermissionRulesBySource::new();

    for (source, rules) in allow_rules.iter_mut() {
        let mut safe = Vec::with_capacity(rules.len());
        let mut dangerous = Vec::new();

        for rule in rules.drain(..) {
            if rule.behavior == PermissionBehavior::Allow
                && (is_dangerous_bash_permission(
                    &rule.value.tool_pattern,
                    rule.value.rule_content.as_deref(),
                    is_ant_user,
                ) || is_dangerous_powershell_permission(
                    &rule.value.tool_pattern,
                    rule.value.rule_content.as_deref(),
                ) || is_dangerous_agent_permission(&rule.value.tool_pattern))
            {
                dangerous.push(rule);
            } else {
                safe.push(rule);
            }
        }

        *rules = safe;
        // Only stash rules from restorable sources. Policy/flag/command rules
        // are removed but NOT restored on exit (restoring them would bypass
        // enterprise controls).
        if !dangerous.is_empty() && is_restorable_source(*source) {
            stripped.entry(*source).or_default().extend(dangerous);
        }
    }

    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

/// Strip dangerous classifier-bypassing rules from the permission context,
/// stashing the removed (restorable) rules in `stripped_dangerous_rules`.
pub fn strip_dangerous_rules(context: &mut ToolPermissionContext, is_ant_user: bool) {
    if let Some(stripped) = strip_dangerous_allow_rules(&mut context.allow_rules, is_ant_user) {
        context.stripped_dangerous_rules = Some(stripped);
    }
}

/// Restore previously stripped dangerous rules.
///
/// Moves rules from `stripped_dangerous_rules` back into `allow_rules`.
/// Clears the stash after restoration.
pub fn restore_dangerous_rules(context: &mut ToolPermissionContext) {
    if let Some(stripped) = context.stripped_dangerous_rules.take() {
        for (source, rules) in stripped {
            context.allow_rules.entry(source).or_default().extend(rules);
        }
    }
}

/// Any Agent allow rule bypasses the classifier's sub-agent evaluation.
fn is_dangerous_agent_permission(tool_name: &str) -> bool {
    tool_name == ToolName::Agent.as_str()
}

/// Whether a rule source is restorable on auto-mode exit.
///
/// Policy/flag/command rules are removed but not stashed for restoration,
/// since restoring them would bypass enterprise controls.
fn is_restorable_source(source: PermissionRuleSource) -> bool {
    matches!(
        source,
        PermissionRuleSource::UserSettings
            | PermissionRuleSource::ProjectSettings
            | PermissionRuleSource::LocalSettings
            | PermissionRuleSource::Session
            | PermissionRuleSource::CliArg
    )
}

#[cfg(test)]
#[path = "dangerous_rules.test.rs"]
mod tests;
