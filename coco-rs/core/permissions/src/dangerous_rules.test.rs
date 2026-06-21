use std::collections::HashMap;

use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolPermissionContext;

use super::*;

fn make_context_with_rules(rules: Vec<PermissionRule>) -> ToolPermissionContext {
    let mut allow_rules: PermissionRulesBySource = HashMap::new();
    for rule in rules {
        allow_rules.entry(rule.source).or_default().push(rule);
    }
    ToolPermissionContext {
        mode: coco_types::PermissionMode::Auto,
        additional_dirs: HashMap::new(),
        permission_rule_source_roots: HashMap::new(),
        allow_rules,
        deny_rules: HashMap::new(),
        ask_rules: HashMap::new(),
        bypass_available: false,
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        session_plan_file: None,
    }
}

fn allow_rule(tool: &str, content: Option<&str>) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::UserSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: content.map(String::from),
        },
    }
}

#[test]
fn test_strip_dangerous_bash_rules() {
    let mut ctx = make_context_with_rules(vec![
        allow_rule("Read", None),             // safe
        allow_rule("Bash", Some("git *")),    // safe (not dangerous)
        allow_rule("Bash", Some("python *")), // dangerous
        allow_rule("Bash", Some("node *")),   // dangerous
    ]);

    strip_dangerous_rules(&mut ctx, /*is_ant*/ false);

    // safe rules remain
    let has_read = ctx
        .allow_rules
        .values()
        .flatten()
        .any(|r| r.value.tool_pattern == "Read");
    assert!(has_read);

    // dangerous rules stashed
    let stripped = ctx
        .stripped_dangerous_rules
        .as_ref()
        .expect("should have stripped");
    let stripped_patterns: Vec<_> = stripped
        .values()
        .flatten()
        .filter_map(|r| r.value.rule_content.as_deref())
        .collect();
    assert!(stripped_patterns.contains(&"python *"));
    assert!(stripped_patterns.contains(&"node *"));
}

#[test]
fn test_restore_dangerous_rules() {
    let mut ctx = make_context_with_rules(vec![
        allow_rule("Read", None),
        allow_rule("Bash", Some("python *")),
    ]);

    strip_dangerous_rules(&mut ctx, false);
    assert!(ctx.stripped_dangerous_rules.is_some());

    // Count rules before restore
    let before_count: usize = ctx.allow_rules.values().map(Vec::len).sum();

    restore_dangerous_rules(&mut ctx);

    // Rules restored
    let after_count: usize = ctx.allow_rules.values().map(Vec::len).sum();
    assert!(after_count > before_count);
    assert!(ctx.stripped_dangerous_rules.is_none());
}

#[test]
fn test_strip_restore_round_trip() {
    let rules = vec![
        allow_rule("Read", None),
        allow_rule("Bash", Some("python *")),
        allow_rule("Bash", Some("git *")),
    ];
    let mut ctx = make_context_with_rules(rules);
    let original_count: usize = ctx.allow_rules.values().map(Vec::len).sum();

    strip_dangerous_rules(&mut ctx, false);
    restore_dangerous_rules(&mut ctx);

    let final_count: usize = ctx.allow_rules.values().map(Vec::len).sum();
    assert_eq!(original_count, final_count);
}

#[test]
fn test_strip_nothing_dangerous() {
    let mut ctx = make_context_with_rules(vec![
        allow_rule("Read", None),
        allow_rule("Bash", Some("git *")),
    ]);

    strip_dangerous_rules(&mut ctx, false);
    assert!(ctx.stripped_dangerous_rules.is_none());
}

#[test]
fn test_restore_when_nothing_stripped() {
    let mut ctx = make_context_with_rules(vec![allow_rule("Read", None)]);

    // Restore with no stripped rules — should be a no-op.
    restore_dangerous_rules(&mut ctx);
    assert!(ctx.stripped_dangerous_rules.is_none());
}
