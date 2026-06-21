use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use pretty_assertions::assert_eq;

use super::*;

// ── escape / unescape ──

#[test]
fn test_escape_round_trips() {
    let original = r#"python -c "print(1)"#;
    let escaped = escape_rule_content(original);
    let unescaped = unescape_rule_content(&escaped);
    assert_eq!(unescaped, original);
}

#[test]
fn test_escape_backslashes_first() {
    let input = r#"echo "test\nvalue""#;
    let escaped = escape_rule_content(input);
    assert!(escaped.contains("\\\\"));
    let unescaped = unescape_rule_content(&escaped);
    assert_eq!(unescaped, input);
}

// ── parse_rule_string ──

#[test]
fn test_parse_simple_tool() {
    let value = parse_rule_string("Bash");
    assert_eq!(value.tool_pattern, "Bash");
    assert_eq!(value.rule_content, None);
}

#[test]
fn test_parse_tool_with_content() {
    let value = parse_rule_string("Bash(npm install)");
    assert_eq!(value.tool_pattern, "Bash");
    assert_eq!(value.rule_content.as_deref(), Some("npm install"));
}

#[test]
fn test_parse_tool_empty_parens_is_tool_wide() {
    let value = parse_rule_string("Bash()");
    assert_eq!(value.tool_pattern, "Bash");
    assert_eq!(value.rule_content, None);
}

#[test]
fn test_parse_normalizes_legacy_tool_names() {
    // P9: legacy tool names normalize to canonical at parse time so the rules
    // match the real tools.
    assert_eq!(parse_rule_string("Task").tool_pattern, "Agent");
    let agent_typed = parse_rule_string("Task(Explore)");
    assert_eq!(agent_typed.tool_pattern, "Agent");
    assert_eq!(agent_typed.rule_content.as_deref(), Some("Explore"));
    assert_eq!(parse_rule_string("KillShell").tool_pattern, "TaskStop");
    assert_eq!(
        parse_rule_string("AgentOutputTool").tool_pattern,
        "TaskOutput"
    );
    assert_eq!(
        parse_rule_string("BashOutputTool").tool_pattern,
        "TaskOutput"
    );
    // Canonical / non-alias names are untouched.
    assert_eq!(parse_rule_string("Bash").tool_pattern, "Bash");
    assert_eq!(parse_rule_string("Read(/x)").tool_pattern, "Read");
}

#[test]
fn test_parse_tool_wildcard_parens_is_tool_wide() {
    let value = parse_rule_string("Bash(*)");
    assert_eq!(value.tool_pattern, "Bash");
    assert_eq!(value.rule_content, None);
}

#[test]
fn test_parse_tool_with_escaped_parens() {
    let value = parse_rule_string(r"Bash(python -c print\(1\))");
    assert_eq!(value.tool_pattern, "Bash");
    assert_eq!(value.rule_content.as_deref(), Some("python -c print(1)"));
}

#[test]
fn test_parse_malformed_no_close_paren() {
    let value = parse_rule_string("Bash(no close");
    assert_eq!(value.tool_pattern, "Bash(no close");
    assert_eq!(value.rule_content, None);
}

#[test]
fn test_parse_missing_tool_name() {
    // "(foo)" is malformed — treated as the whole string being the tool name
    let value = parse_rule_string("(foo)");
    assert_eq!(value.tool_pattern, "(foo)");
    assert_eq!(value.rule_content, None);
}

// ── rule_value_to_string ──

#[test]
fn test_rule_value_round_trip() {
    let value = PermissionRuleValue {
        tool_pattern: "Bash".to_string(),
        rule_content: Some("git *".to_string()),
    };
    let s = rule_value_to_string(&value);
    assert_eq!(s, "Bash(git *)");
    let parsed = parse_rule_string(&s);
    assert_eq!(parsed.tool_pattern, "Bash");
    assert_eq!(parsed.rule_content.as_deref(), Some("git *"));
}

#[test]
fn test_rule_value_to_string_no_content() {
    let value = PermissionRuleValue {
        tool_pattern: "Read".to_string(),
        rule_content: None,
    };
    assert_eq!(rule_value_to_string(&value), "Read");
}

// ── compile_rules ──

#[test]
fn test_compile_rules_basic() {
    let entries = vec![
        (
            PermissionRuleSource::Session,
            PermissionBehavior::Allow,
            "Read",
        ),
        (
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Deny,
            "Bash(rm -rf *)",
        ),
        (
            PermissionRuleSource::CliArg,
            PermissionBehavior::Allow,
            "Bash(git *)",
        ),
    ];

    let rules = compile_rules(&entries);
    assert_eq!(rules.len(), 3);
    assert_eq!(rules[0].value.tool_pattern, "Read");
    assert_eq!(rules[0].behavior, PermissionBehavior::Allow);

    assert_eq!(rules[1].value.tool_pattern, "Bash");
    assert_eq!(rules[1].value.rule_content.as_deref(), Some("rm -rf *"));
    assert_eq!(rules[1].behavior, PermissionBehavior::Deny);

    assert_eq!(rules[2].value.tool_pattern, "Bash");
    assert_eq!(rules[2].value.rule_content.as_deref(), Some("git *"));
}

// ── evaluate_rules_for_tool ──

#[test]
fn test_evaluate_deny_wins_over_allow() {
    let rules = compile_rules(&[
        (
            PermissionRuleSource::Session,
            PermissionBehavior::Allow,
            "Bash",
        ),
        (
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Deny,
            "Bash",
        ),
    ]);

    let result = evaluate_rules_for_tool(&rules, "Bash", None);
    assert!(result.matched);
    assert_eq!(result.behavior, Some(PermissionBehavior::Deny));
}

#[test]
fn test_evaluate_content_match() {
    let rules = compile_rules(&[(
        PermissionRuleSource::Session,
        PermissionBehavior::Allow,
        "Bash(git *)",
    )]);

    // git status matches "git *"
    let result = evaluate_rules_for_tool(&rules, "Bash", Some("git status"));
    assert!(result.matched);
    assert!(result.content_match);

    // rm -rf does NOT match "git *"
    let result = evaluate_rules_for_tool(&rules, "Bash", Some("rm -rf /"));
    assert!(!result.matched);
}

#[test]
fn test_evaluate_no_match() {
    let rules = compile_rules(&[(
        PermissionRuleSource::Session,
        PermissionBehavior::Allow,
        "Read",
    )]);

    let result = evaluate_rules_for_tool(&rules, "Bash", None);
    assert!(!result.matched);
    assert!(result.rule.is_none());
}

#[test]
fn test_evaluate_mcp_server_level() {
    let rules = compile_rules(&[(
        PermissionRuleSource::Session,
        PermissionBehavior::Allow,
        "mcp__slack",
    )]);

    let result = evaluate_rules_for_tool(&rules, "mcp__slack__send_message", None);
    assert!(result.matched);
    assert_eq!(result.behavior, Some(PermissionBehavior::Allow));
}

#[test]
fn test_evaluate_mcp_wildcard() {
    let rules = compile_rules(&[(
        PermissionRuleSource::Session,
        PermissionBehavior::Allow,
        "mcp__github__*",
    )]);

    let result = evaluate_rules_for_tool(&rules, "mcp__github__create_issue", None);
    assert!(result.matched);
}

#[test]
fn test_evaluate_ask_falls_through() {
    let rules = compile_rules(&[(
        PermissionRuleSource::Session,
        PermissionBehavior::Ask,
        "Bash",
    )]);

    let result = evaluate_rules_for_tool(&rules, "Bash", None);
    assert!(result.matched);
    assert_eq!(result.behavior, Some(PermissionBehavior::Ask));
}

// ── tool_matches_pattern ──

#[test]
fn test_tool_matches_global_wildcard() {
    assert!(tool_matches_pattern("*", "anything"));
    assert!(tool_matches_pattern("*", "mcp__server__tool"));
}

#[test]
fn test_tool_matches_exact() {
    assert!(tool_matches_pattern("Read", "Read"));
    assert!(!tool_matches_pattern("Read", "Write"));
}

#[test]
fn test_tool_matches_prefix_wildcard() {
    assert!(tool_matches_pattern("mcp__slack__*", "mcp__slack__send"));
    assert!(!tool_matches_pattern("mcp__slack__*", "mcp__github__list"));
}
