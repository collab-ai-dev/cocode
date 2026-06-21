use std::collections::HashMap;

use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::ToolPermissionContext;

use super::*;

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

fn make_allow_rule(tool: &str, content: Option<&str>) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::LocalSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: content.map(String::from),
        },
    }
}

// ── SetMode ──

#[test]
fn test_set_mode() {
    let ctx = empty_context();
    let update = PermissionUpdate::SetMode {
        mode: PermissionMode::Auto,
    };
    let result = apply_permission_update(ctx, &update);
    assert_eq!(result.mode, PermissionMode::Auto);
}

// ── AddRules ──

#[test]
fn test_add_allow_rules() {
    let ctx = empty_context();
    let update = PermissionUpdate::AddRules {
        rules: vec![make_allow_rule("Bash", Some("git *"))],
        destination: PermissionUpdateDestination::LocalSettings,
    };
    let result = apply_permission_update(ctx, &update);

    let rules = result
        .allow_rules
        .get(&PermissionRuleSource::LocalSettings)
        .expect("should have local rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].value.tool_pattern, "Bash");
}

// ── RemoveRules ──

#[test]
fn test_remove_rules() {
    let mut ctx = empty_context();
    let rule = make_allow_rule("Bash", Some("git *"));
    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(rule.clone());

    let update = PermissionUpdate::RemoveRules {
        rules: vec![rule],
        destination: PermissionUpdateDestination::LocalSettings,
    };
    let result = apply_permission_update(ctx, &update);

    let rules = result
        .allow_rules
        .get(&PermissionRuleSource::LocalSettings)
        .expect("should have entry");
    assert!(rules.is_empty());
}

// ── ReplaceRules ──

#[test]
fn test_replace_rules() {
    let mut ctx = empty_context();
    ctx.allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(make_allow_rule("Bash", Some("rm *")));

    let update = PermissionUpdate::ReplaceRules {
        rules: vec![make_allow_rule("Bash", Some("ls *"))],
        destination: PermissionUpdateDestination::LocalSettings,
    };
    let result = apply_permission_update(ctx, &update);

    let rules = result
        .allow_rules
        .get(&PermissionRuleSource::LocalSettings)
        .expect("should have rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].value.rule_content.as_deref(), Some("ls *"));
}

// ── AddDirectories ──

#[test]
fn test_add_directories() {
    let ctx = empty_context();
    let update = PermissionUpdate::AddDirectories {
        directories: vec!["/opt/data".to_string()],
        destination: PermissionUpdateDestination::Session,
    };
    let result = apply_permission_update(ctx, &update);

    assert!(result.additional_dirs.contains_key("/opt/data"));
    assert_eq!(result.additional_dirs["/opt/data"].path, "/opt/data");
}

// ── RemoveDirectories ──

#[test]
fn test_remove_directories() {
    let mut ctx = empty_context();
    ctx.additional_dirs.insert(
        "/opt/data".to_string(),
        AdditionalWorkingDir {
            path: "/opt/data".to_string(),
            source: PermissionUpdateDestination::Session,
        },
    );

    let update = PermissionUpdate::RemoveDirectories {
        directories: vec!["/opt/data".to_string()],
        destination: PermissionUpdateDestination::Session,
    };
    let result = apply_permission_update(ctx, &update);

    assert!(!result.additional_dirs.contains_key("/opt/data"));
}

// ── Multiple updates ──

#[test]
fn test_apply_multiple_updates() {
    let ctx = empty_context();
    let updates = vec![
        PermissionUpdate::SetMode {
            mode: PermissionMode::AcceptEdits,
        },
        PermissionUpdate::AddRules {
            rules: vec![make_allow_rule("Bash", Some("git *"))],
            destination: PermissionUpdateDestination::Session,
        },
        PermissionUpdate::AddDirectories {
            directories: vec!["/tmp/work".to_string()],
            destination: PermissionUpdateDestination::Session,
        },
    ];
    let result = apply_permission_updates(ctx, &updates);

    assert_eq!(result.mode, PermissionMode::AcceptEdits);
    assert!(result.additional_dirs.contains_key("/tmp/work"));

    let session_rules = result
        .allow_rules
        .get(&PermissionRuleSource::Session)
        .expect("should have session rules");
    assert_eq!(session_rules.len(), 1);
}

// ── Utilities ──

#[test]
fn test_supports_persistence() {
    assert!(supports_persistence(
        PermissionUpdateDestination::LocalSettings
    ));
    assert!(supports_persistence(
        PermissionUpdateDestination::UserSettings
    ));
    assert!(supports_persistence(
        PermissionUpdateDestination::ProjectSettings
    ));
    assert!(!supports_persistence(PermissionUpdateDestination::Session));
    assert!(!supports_persistence(PermissionUpdateDestination::CliArg));
    // Skill-driven rules are in-memory; they belong to the `Command`
    // bucket which never persists (session-lifetime only).
    assert!(!supports_persistence(PermissionUpdateDestination::Command));
}

#[test]
fn test_command_destination_routes_to_command_source() {
    // Adding a rule with `destination: Command` must land it under
    // `PermissionRuleSource::Command` in the allow map. This is the
    // central guarantee skill auto-allow relies on — without it,
    // skills' `allowed-tools` would silently disappear or pollute the
    // wrong bucket.
    let ctx = empty_context();
    let update = PermissionUpdate::AddRules {
        rules: vec![make_allow_rule("Read", None)],
        destination: PermissionUpdateDestination::Command,
    };
    let updated = apply_permission_update(ctx, &update);
    let command_rules = updated
        .allow_rules
        .get(&PermissionRuleSource::Command)
        .expect("Command source must be populated");
    assert_eq!(command_rules.len(), 1);
    assert_eq!(command_rules[0].value.tool_pattern, "Read");
}

#[test]
fn test_has_rules_true() {
    let updates = vec![PermissionUpdate::AddRules {
        rules: vec![make_allow_rule("Bash", None)],
        destination: PermissionUpdateDestination::Session,
    }];
    assert!(has_rules(&updates));
}

#[test]
fn test_has_rules_false_for_mode() {
    let updates = vec![PermissionUpdate::SetMode {
        mode: PermissionMode::Auto,
    }];
    assert!(!has_rules(&updates));
}
