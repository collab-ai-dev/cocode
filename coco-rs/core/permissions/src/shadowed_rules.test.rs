use std::collections::HashMap;

use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolPermissionContext;

use super::*;

fn make_rule(
    tool: &str,
    content: Option<&str>,
    behavior: PermissionBehavior,
    source: PermissionRuleSource,
) -> PermissionRule {
    PermissionRule {
        source,
        behavior,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: content.map(String::from),
        },
    }
}

fn empty_context() -> ToolPermissionContext {
    ToolPermissionContext {
        mode: PermissionMode::Default,
        additional_dirs: HashMap::new(),
        permission_rule_source_roots: HashMap::new(),
        allow_rules: HashMap::new(),
        deny_rules: HashMap::new(),
        ask_rules: HashMap::new(),
        bypass_available: false,
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        session_plan_file: None,
    }
}

fn default_options() -> DetectUnreachableRulesOptions {
    DetectUnreachableRulesOptions {
        sandbox_auto_allow_enabled: false,
    }
}

// ── Deny shadowing ──

#[test]
fn test_allow_rule_shadowed_by_deny() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("git *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    let deny = make_rule(
        "Bash",
        None,
        PermissionBehavior::Deny,
        PermissionRuleSource::ProjectSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.deny_rules
        .entry(PermissionRuleSource::ProjectSettings)
        .or_default()
        .push(deny);

    let unreachable = detect_unreachable_rules(&ctx, &default_options());
    assert_eq!(unreachable.len(), 1);
    assert_eq!(unreachable[0].shadow_type, ShadowType::Deny);
    assert!(unreachable[0].reason.contains("deny rule"));
}

// ── Ask shadowing ──

#[test]
fn test_allow_rule_shadowed_by_ask() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("ls *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    let ask = make_rule(
        "Bash",
        None,
        PermissionBehavior::Ask,
        PermissionRuleSource::UserSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.ask_rules
        .entry(PermissionRuleSource::UserSettings)
        .or_default()
        .push(ask);

    let unreachable = detect_unreachable_rules(&ctx, &default_options());
    assert_eq!(unreachable.len(), 1);
    assert_eq!(unreachable[0].shadow_type, ShadowType::Ask);
}

// ── Tool-wide allow not shadowed ──

#[test]
fn test_tool_wide_allow_not_shadowed() {
    let mut ctx = empty_context();

    // Tool-wide allow (no content) cannot be shadowed
    let allow = make_rule(
        "Bash",
        None,
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    let deny = make_rule(
        "Bash",
        None,
        PermissionBehavior::Deny,
        PermissionRuleSource::ProjectSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.deny_rules
        .entry(PermissionRuleSource::ProjectSettings)
        .or_default()
        .push(deny);

    let unreachable = detect_unreachable_rules(&ctx, &default_options());
    assert!(unreachable.is_empty());
}

// ── Bash sandbox exception ──

#[test]
fn test_bash_sandbox_exception_personal_settings() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("git *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    // Ask from personal settings (userSettings) — sandbox exception applies
    let ask = make_rule(
        "Bash",
        None,
        PermissionBehavior::Ask,
        PermissionRuleSource::UserSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.ask_rules
        .entry(PermissionRuleSource::UserSettings)
        .or_default()
        .push(ask);

    let options = DetectUnreachableRulesOptions {
        sandbox_auto_allow_enabled: true,
    };

    // With sandbox enabled, personal ask rule doesn't shadow
    let unreachable = detect_unreachable_rules(&ctx, &options);
    assert!(unreachable.is_empty());
}

#[test]
fn test_bash_sandbox_exception_not_for_shared() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("git *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    // Ask from shared settings (projectSettings) — no sandbox exception
    let ask = make_rule(
        "Bash",
        None,
        PermissionBehavior::Ask,
        PermissionRuleSource::ProjectSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.ask_rules
        .entry(PermissionRuleSource::ProjectSettings)
        .or_default()
        .push(ask);

    let options = DetectUnreachableRulesOptions {
        sandbox_auto_allow_enabled: true,
    };

    // Shared settings always warn even with sandbox
    let unreachable = detect_unreachable_rules(&ctx, &options);
    assert_eq!(unreachable.len(), 1);
    assert_eq!(unreachable[0].shadow_type, ShadowType::Ask);
}

// ── Deny takes precedence over ask report ──

#[test]
fn test_deny_and_ask_only_reports_deny() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("rm *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    let deny = make_rule(
        "Bash",
        None,
        PermissionBehavior::Deny,
        PermissionRuleSource::PolicySettings,
    );
    let ask = make_rule(
        "Bash",
        None,
        PermissionBehavior::Ask,
        PermissionRuleSource::UserSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.deny_rules
        .entry(PermissionRuleSource::PolicySettings)
        .or_default()
        .push(deny);
    ctx.ask_rules
        .entry(PermissionRuleSource::UserSettings)
        .or_default()
        .push(ask);

    let unreachable = detect_unreachable_rules(&ctx, &default_options());
    assert_eq!(unreachable.len(), 1);
    assert_eq!(unreachable[0].shadow_type, ShadowType::Deny);
}

// ── No shadowing ──

#[test]
fn test_no_shadowing_when_different_tools() {
    let mut ctx = empty_context();

    let allow = make_rule(
        "Bash",
        Some("git *"),
        PermissionBehavior::Allow,
        PermissionRuleSource::LocalSettings,
    );
    let deny = make_rule(
        "Write",
        None,
        PermissionBehavior::Deny,
        PermissionRuleSource::ProjectSettings,
    );

    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);
    ctx.deny_rules
        .entry(PermissionRuleSource::ProjectSettings)
        .or_default()
        .push(deny);

    let unreachable = detect_unreachable_rules(&ctx, &default_options());
    assert!(unreachable.is_empty());
}
