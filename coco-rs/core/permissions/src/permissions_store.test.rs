use coco_types::PermissionMode;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;

use super::*;

fn make_rule(
    tool: &str,
    behavior: PermissionBehavior,
    source: PermissionRuleSource,
) -> PermissionRule {
    PermissionRule {
        source,
        behavior,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: None,
        },
    }
}

#[test]
fn test_into_context_groups_by_source() {
    let rules = PermissionRulesByBehavior {
        allow: vec![
            make_rule(
                "Read",
                PermissionBehavior::Allow,
                PermissionRuleSource::UserSettings,
            ),
            make_rule(
                "Bash",
                PermissionBehavior::Allow,
                PermissionRuleSource::LocalSettings,
            ),
        ],
        deny: vec![make_rule(
            "Write",
            PermissionBehavior::Deny,
            PermissionRuleSource::PolicySettings,
        )],
        ask: vec![],
    };

    let ctx = rules.into_context(PermissionMode::Default);

    assert_eq!(ctx.allow_rules.len(), 2); // 2 different sources
    assert_eq!(ctx.deny_rules.len(), 1);
    assert!(ctx.ask_rules.is_empty());
    assert_eq!(ctx.mode, PermissionMode::Default);
}

#[test]
fn test_parse_behavior() {
    assert_eq!(parse_behavior("allow"), Some(PermissionBehavior::Allow));
    assert_eq!(parse_behavior("deny"), Some(PermissionBehavior::Deny));
    assert_eq!(parse_behavior("ask"), Some(PermissionBehavior::Ask));
    assert_eq!(parse_behavior("other"), None);
}
