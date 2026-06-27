use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolName;

use super::*;

// ── compute_auto_mode_capability ──

#[test]
fn test_compute_auto_mode_capability_default_on() {
    // No settings opt-out → Auto is available by default.
    assert!(compute_auto_mode_capability(
        /*settings_disabled*/ false
    ));
}

#[test]
fn test_compute_auto_mode_capability_settings_disable() {
    // `auto_mode.disabled = true` hides Auto from the cycle.
    assert!(!compute_auto_mode_capability(
        /*settings_disabled*/ true
    ));
}

// ── permission_mode_title ──

#[test]
fn test_permission_mode_title_all_modes() {
    assert_eq!(permission_mode_title(PermissionMode::Default), "Default");
    assert_eq!(permission_mode_title(PermissionMode::Plan), "Plan Mode");
    assert_eq!(
        permission_mode_title(PermissionMode::AcceptEdits),
        "Accept edits"
    );
    assert_eq!(
        permission_mode_title(PermissionMode::BypassPermissions),
        "Bypass Permissions"
    );
    assert_eq!(permission_mode_title(PermissionMode::DontAsk), "Don't Ask");
    assert_eq!(permission_mode_title(PermissionMode::Auto), "Auto mode");
}

#[test]
fn test_permission_mode_description_non_empty() {
    let modes = [
        PermissionMode::Default,
        PermissionMode::Plan,
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
        PermissionMode::DontAsk,
        PermissionMode::Auto,
        PermissionMode::Bubble,
    ];
    for mode in modes {
        let desc = permission_mode_description(mode);
        assert!(
            !desc.is_empty(),
            "description for {mode:?} should not be empty"
        );
    }
}

// ── is_default_mode ──

#[test]
fn test_is_default_mode() {
    assert!(is_default_mode(None));
    assert!(is_default_mode(Some(PermissionMode::Default)));
    assert!(!is_default_mode(Some(PermissionMode::Plan)));
    assert!(!is_default_mode(Some(PermissionMode::BypassPermissions)));
}

// ── is_dangerous_bash_permission ──

#[test]
fn test_dangerous_bash_no_content() {
    assert!(is_dangerous_bash_permission(
        "Bash", None, /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some(""),
        /*is_ant*/ false
    ));
}

#[test]
fn test_dangerous_bash_wildcard() {
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("*"),
        /*is_ant*/ false
    ));
}

#[test]
fn test_dangerous_bash_interpreter_patterns() {
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("python:*"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("node*"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("ruby *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("python -c *"),
        /*is_ant*/ false
    ));
}

#[test]
fn test_dangerous_bash_ts_aligned_patterns() {
    // Patterns from CROSS_PLATFORM_CODE_EXEC
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("python2 *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("tsx *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("bunx *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("ssh *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("npm run *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("yarn run *"),
        /*is_ant*/ false
    ));
    // Bash-specific
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("env *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("xargs *"),
        /*is_ant*/ false
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("sudo *"),
        /*is_ant*/ false
    ));
}

#[test]
fn test_dangerous_bash_ant_only_patterns() {
    // These should NOT be dangerous for non-ant users
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("git *"),
        /*is_ant*/ false
    ));
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("curl *"),
        /*is_ant*/ false
    ));
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("gh *"),
        /*is_ant*/ false
    ));
    // But should be dangerous for ant users
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("git *"),
        /*is_ant*/ true
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("curl *"),
        /*is_ant*/ true
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("gh *"),
        /*is_ant*/ true
    ));
    assert!(is_dangerous_bash_permission(
        "Bash",
        Some("kubectl *"),
        /*is_ant*/ true
    ));
}

#[test]
fn test_safe_bash_patterns() {
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("git status"),
        /*is_ant*/ false
    ));
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("ls -la"),
        /*is_ant*/ false
    ));
    assert!(!is_dangerous_bash_permission(
        "Bash",
        Some("cat README.md"),
        /*is_ant*/ false
    ));
}

#[test]
fn test_non_bash_not_dangerous() {
    assert!(!is_dangerous_bash_permission(
        "Read", None, /*is_ant*/ false
    ));
    assert!(!is_dangerous_bash_permission(
        "Edit",
        Some("*"),
        /*is_ant*/ false
    ));
}

// ── is_dangerous_powershell_permission ──

#[test]
fn test_dangerous_ps_no_content() {
    assert!(is_dangerous_powershell_permission("PowerShell", None));
    assert!(is_dangerous_powershell_permission("PowerShell", Some("")));
}

#[test]
fn test_dangerous_ps_wildcard() {
    assert!(is_dangerous_powershell_permission("PowerShell", Some("*")));
}

#[test]
fn test_dangerous_ps_cross_platform() {
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("python *")
    ));
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("node *")
    ));
}

#[test]
fn test_dangerous_ps_specific_patterns() {
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("invoke-expression *")
    ));
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("start-process *")
    ));
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("add-type *")
    ));
}

#[test]
fn test_dangerous_ps_exe_variant() {
    // "npm run" → "npm.exe run" should also be caught
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("npm.exe run *")
    ));
    assert!(is_dangerous_powershell_permission(
        "PowerShell",
        Some("python.exe *")
    ));
}

#[test]
fn test_non_ps_not_dangerous() {
    assert!(!is_dangerous_powershell_permission("Bash", Some("*")));
    assert!(!is_dangerous_powershell_permission("Read", None));
}

// ── find_dangerous_classifier_permissions ──

#[test]
fn test_find_dangerous_permissions_from_rules() {
    let rules = vec![
        PermissionRule {
            source: PermissionRuleSource::UserSettings,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Bash".to_string(),
                rule_content: None,
            },
        },
        PermissionRule {
            source: PermissionRuleSource::UserSettings,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Read".to_string(),
                rule_content: None,
            },
        },
    ];
    let result = find_dangerous_classifier_permissions(&rules, &[], /*is_ant*/ false);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].rule_display, "Bash(*)");
}

#[test]
fn test_find_dangerous_permissions_from_cli() {
    let cli_tools = vec!["Bash".to_string(), "Bash(python:*)".to_string()];
    let result = find_dangerous_classifier_permissions(&[], &cli_tools, /*is_ant*/ false);
    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|r| r.source_display == "--allowed-tools"));
}

#[test]
fn test_find_dangerous_permissions_deny_not_flagged() {
    let rules = vec![PermissionRule {
        source: PermissionRuleSource::UserSettings,
        behavior: PermissionBehavior::Deny,
        value: PermissionRuleValue {
            tool_pattern: "Bash".to_string(),
            rule_content: None,
        },
    }];
    let result = find_dangerous_classifier_permissions(&rules, &[], /*is_ant*/ false);
    assert!(
        result.is_empty(),
        "deny rules should not be flagged as dangerous"
    );
}

#[test]
fn test_find_dangerous_ps_permissions() {
    let rules = vec![PermissionRule {
        source: PermissionRuleSource::UserSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "PowerShell".to_string(),
            rule_content: None,
        },
    }];
    let result = find_dangerous_classifier_permissions(&rules, &[], /*is_ant*/ false);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].rule_display, "PowerShell(*)");
}

// ── parse_tool_spec ──

#[test]
fn test_parse_tool_spec_simple() {
    let (name, content) = parse_tool_spec("Bash");
    assert_eq!(name, "Bash");
    assert_eq!(content, None);
}

#[test]
fn test_parse_tool_spec_with_content() {
    let (name, content) = parse_tool_spec("Bash(git *)");
    assert_eq!(name, "Bash");
    assert_eq!(content, Some("git *"));
}

#[test]
fn test_parse_tool_spec_empty_parens() {
    let (name, content) = parse_tool_spec("Bash()");
    assert_eq!(name, "Bash");
    assert_eq!(content, None);
}

// ── default_session_rules ──

#[test]
fn test_default_session_rules_all_read_only() {
    let rules = default_session_rules();
    assert!(!rules.is_empty());
    for rule in &rules {
        assert_eq!(rule.behavior, PermissionBehavior::Allow);
        assert_eq!(rule.source, PermissionRuleSource::Session);
    }
    let tool_names: Vec<&str> = rules
        .iter()
        .map(|r| r.value.tool_pattern.as_str())
        .collect();
    assert!(tool_names.contains(&ToolName::Read.as_str()));
    assert!(tool_names.contains(&ToolName::Glob.as_str()));
    assert!(tool_names.contains(&ToolName::Grep.as_str()));
}

// ── resolve_permission_mode ──

#[test]
fn test_resolve_mode_defaults() {
    assert_eq!(
        resolve_permission_mode(None, None, /*plan_mode*/ false),
        PermissionMode::Default
    );
}

#[test]
fn test_resolve_mode_plan_wins() {
    assert_eq!(
        resolve_permission_mode(
            Some(PermissionMode::BypassPermissions),
            Some(PermissionMode::AcceptEdits),
            /*plan_mode*/ true
        ),
        PermissionMode::Plan
    );
}

#[test]
fn test_resolve_mode_cli_over_settings() {
    assert_eq!(
        resolve_permission_mode(
            Some(PermissionMode::Default),
            Some(PermissionMode::AcceptEdits),
            /*plan_mode*/ false
        ),
        PermissionMode::AcceptEdits
    );
}

#[test]
fn test_resolve_mode_settings_fallback() {
    assert_eq!(
        resolve_permission_mode(
            Some(PermissionMode::BypassPermissions),
            None,
            /*plan_mode*/ false
        ),
        PermissionMode::BypassPermissions
    );
}

// ── PermissionModeChoice ──

#[test]
fn test_mode_choice_to_permission_mode() {
    assert_eq!(
        PermissionModeChoice::Interactive.to_permission_mode(),
        PermissionMode::Default
    );
    assert_eq!(
        PermissionModeChoice::Auto.to_permission_mode(),
        PermissionMode::Auto
    );
    assert_eq!(
        PermissionModeChoice::Plan.to_permission_mode(),
        PermissionMode::Plan
    );
    assert_eq!(
        PermissionModeChoice::Bypass.to_permission_mode(),
        PermissionMode::BypassPermissions
    );
}

#[test]
fn test_mode_choice_labels_non_empty() {
    for choice in PermissionModeChoice::ALL {
        assert!(!choice.label().is_empty());
        assert!(!choice.description().is_empty());
    }
}

#[test]
fn test_mode_choice_all_contains_four() {
    assert_eq!(PermissionModeChoice::ALL.len(), 4);
}

// ── get_default_rules_for_mode ──

#[test]
fn test_default_rules_for_default_mode() {
    let rules = get_default_rules_for_mode(PermissionMode::Default);
    assert!(!rules.is_empty());
    for rule in &rules {
        assert_eq!(rule.behavior, PermissionBehavior::Allow);
        assert_eq!(rule.source, PermissionRuleSource::Session);
    }
    let patterns: Vec<&str> = rules
        .iter()
        .map(|r| r.value.tool_pattern.as_str())
        .collect();
    assert!(patterns.contains(&ToolName::Read.as_str()));
    assert!(patterns.contains(&ToolName::Grep.as_str()));
    // Edit should NOT be in default mode
    assert!(!patterns.contains(&ToolName::Edit.as_str()));
}

#[test]
fn test_default_rules_for_accept_edits_mode() {
    let rules = get_default_rules_for_mode(PermissionMode::AcceptEdits);
    let patterns: Vec<&str> = rules
        .iter()
        .map(|r| r.value.tool_pattern.as_str())
        .collect();
    // AcceptEdits should include Edit and Write
    assert!(patterns.contains(&ToolName::Edit.as_str()));
    assert!(patterns.contains(&ToolName::Write.as_str()));
    assert!(patterns.contains(&ToolName::Read.as_str()));
}

#[test]
fn test_default_rules_for_bypass_mode() {
    let rules = get_default_rules_for_mode(PermissionMode::BypassPermissions);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].value.tool_pattern, "*");
    assert_eq!(rules[0].behavior, PermissionBehavior::Allow);
}

#[test]
fn test_default_rules_for_plan_mode() {
    let rules = get_default_rules_for_mode(PermissionMode::Plan);
    let patterns: Vec<&str> = rules
        .iter()
        .map(|r| r.value.tool_pattern.as_str())
        .collect();
    assert!(patterns.contains(&ToolName::Read.as_str()));
    assert!(patterns.contains(&ToolName::EnterPlanMode.as_str()));
    // Bash should NOT be in plan mode
    assert!(!patterns.contains(&ToolName::Bash.as_str()));
}

#[test]
fn test_default_rules_for_bubble_mode() {
    let rules = get_default_rules_for_mode(PermissionMode::Bubble);
    assert!(rules.is_empty());
}

// ── validate_permission_configuration ──

#[test]
fn test_validate_no_conflicts() {
    let rules = vec![PermissionRule {
        source: PermissionRuleSource::Session,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "Read".to_string(),
            rule_content: None,
        },
    }];
    let errors = validate_permission_configuration(
        &rules,
        PermissionMode::Default,
        &[],
        /*is_ant*/ false,
    );
    assert!(errors.is_empty());
}

#[test]
fn test_validate_detects_allow_deny_conflict() {
    let rules = vec![
        PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Bash".to_string(),
                rule_content: None,
            },
        },
        PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Deny,
            value: PermissionRuleValue {
                tool_pattern: "Bash".to_string(),
                rule_content: None,
            },
        },
    ];
    let errors = validate_permission_configuration(
        &rules,
        PermissionMode::Default,
        &[],
        /*is_ant*/ false,
    );
    assert!(!errors.is_empty());
    assert!(errors[0].message.contains("conflicting"));
    assert_eq!(errors[0].severity, "error");
}

#[test]
fn test_validate_auto_mode_warns_dangerous() {
    let rules = vec![PermissionRule {
        source: PermissionRuleSource::UserSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "Bash".to_string(),
            rule_content: None,
        },
    }];
    let errors =
        validate_permission_configuration(&rules, PermissionMode::Auto, &[], /*is_ant*/ false);
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .any(|e| e.severity == "warning" && e.message.contains("dangerous"))
    );
}

#[test]
fn test_validate_empty_tool_pattern() {
    let rules = vec![PermissionRule {
        source: PermissionRuleSource::Session,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "".to_string(),
            rule_content: None,
        },
    }];
    let errors = validate_permission_configuration(
        &rules,
        PermissionMode::Default,
        &[],
        /*is_ant*/ false,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("empty tool pattern"))
    );
}
