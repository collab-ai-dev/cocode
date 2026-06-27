use coco_types::*;
use std::collections::HashMap;

use super::*;

fn empty_context(mode: PermissionMode) -> ToolPermissionContext {
    ToolPermissionContext {
        mode,
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

fn bash_input(command: &str) -> serde_json::Value {
    serde_json::json!({"command": command})
}

fn file_input(path: &str) -> serde_json::Value {
    serde_json::json!({"file_path": path})
}

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

// ── Step 1: Deny rules ──

#[test]
fn test_deny_rule_wins_over_allow() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.deny_rules.insert(
        PermissionRuleSource::UserSettings,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Deny,
            PermissionRuleSource::UserSettings,
        )],
    );
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "*",
            None,
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Deny { .. }));
}

#[test]
fn test_content_specific_deny() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.deny_rules.insert(
        PermissionRuleSource::ProjectSettings,
        vec![make_rule(
            "Bash",
            Some("rm *"),
            PermissionBehavior::Deny,
            PermissionRuleSource::ProjectSettings,
        )],
    );
    // "rm -rf /" matches "rm *" → deny
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("rm -rf /"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Deny { .. }));

    // "ls" does NOT match "rm *" → not denied
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_path_scoped_edit_allow_does_not_allow_unmatched_path() {
    let allowed = tempfile::tempdir().unwrap();
    let denied = tempfile::tempdir().unwrap();
    let denied_file = denied.path().join("data.txt");
    std::fs::write(&denied_file, "data").unwrap();
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "Edit",
            Some(&format!("/{}/**", allowed.path().to_string_lossy())),
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );

    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Edit),
        &file_input(denied_file.to_str().unwrap()),
        &ctx,
    );

    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_path_scoped_edit_allow_allows_matched_path() {
    let allowed = tempfile::tempdir().unwrap();
    let file = allowed.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "Edit",
            Some(&format!("/{}/**", allowed.path().to_string_lossy())),
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );

    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Edit),
        &file_input(file.to_str().unwrap()),
        &ctx,
    );

    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

// ── Step 2-3: Allow rules ──

#[test]
fn test_content_specific_allow_rule() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "Bash",
            Some("git *"),
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("git status"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));

    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("rm -rf /"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_exit_plan_mode_tool_check_ask_is_not_bypassed_by_modes_or_allow_rules() {
    let tool_check =
        |tool_id: &ToolId, _input: &serde_json::Value, _context: &ToolPermissionContext| {
            if tool_id == &ToolId::Builtin(ToolName::ExitPlanMode) {
                ToolCheckResult::Ask {
                    message: "Exit plan mode?".to_string(),
                    suggestions: vec![],
                    choices: None,
                    detail: None,
                }
            } else {
                ToolCheckResult::Passthrough
            }
        };
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Auto,
        PermissionMode::BypassPermissions,
    ] {
        let mut ctx = empty_context(mode);
        ctx.allow_rules.insert(
            PermissionRuleSource::Session,
            vec![make_rule(
                "*",
                None,
                PermissionBehavior::Allow,
                PermissionRuleSource::Session,
            )],
        );
        let result = PermissionEvaluator::evaluate_with_tool_check(
            &ToolId::Builtin(ToolName::ExitPlanMode),
            &serde_json::json!({}),
            &ctx,
            Some(&tool_check),
        );
        assert!(
            matches!(result, PermissionDecision::Ask { .. }),
            "ExitPlanMode tool-check Ask must win in {mode:?}, got {result:?}"
        );
    }
}

#[test]
fn test_exit_plan_mode_tool_check_ask_is_denied_in_dont_ask() {
    let tool_check =
        |_tool_id: &ToolId, _input: &serde_json::Value, _context: &ToolPermissionContext| {
            ToolCheckResult::Ask {
                message: "Exit plan mode?".to_string(),
                suggestions: vec![],
                choices: None,
                detail: None,
            }
        };
    let ctx = empty_context(PermissionMode::DontAsk);
    let result = PermissionEvaluator::evaluate_with_tool_check(
        &ToolId::Builtin(ToolName::ExitPlanMode),
        &serde_json::json!({}),
        &ctx,
        Some(&tool_check),
    );

    assert!(matches!(result, PermissionDecision::Deny { .. }));
}

// ── Step 4: Ask rules (NEW) ──

#[test]
fn test_tool_wide_ask_rule() {
    let mut ctx = empty_context(PermissionMode::BypassPermissions);
    ctx.ask_rules.insert(
        PermissionRuleSource::ProjectSettings,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Ask,
            PermissionRuleSource::ProjectSettings,
        )],
    );
    // Even in bypass mode, tool-wide ask rules force a prompt
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_ask_rule_does_not_affect_other_tools() {
    let mut ctx = empty_context(PermissionMode::BypassPermissions);
    ctx.ask_rules.insert(
        PermissionRuleSource::ProjectSettings,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Ask,
            PermissionRuleSource::ProjectSettings,
        )],
    );
    // Read is not Bash → bypass mode allows it
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Read),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

// ── Step 5: Content-specific ask ──

#[test]
fn test_content_specific_ask_rule() {
    let mut ctx = empty_context(PermissionMode::BypassPermissions);
    ctx.ask_rules.insert(
        PermissionRuleSource::ProjectSettings,
        vec![
            make_rule(
                "Bash",
                None,
                PermissionBehavior::Ask,
                PermissionRuleSource::ProjectSettings,
            ),
            make_rule(
                "Bash",
                Some("rm *"),
                PermissionBehavior::Ask,
                PermissionRuleSource::ProjectSettings,
            ),
        ],
    );
    // "rm -rf" matches content-specific ask → ask
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("rm -rf /"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

// ── Step 6: Path safety ──

#[test]
fn test_write_to_dangerous_path_asks() {
    let ctx = empty_context(PermissionMode::BypassPermissions);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("/home/user/.bashrc"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_write_to_safe_path_allowed_in_bypass() {
    let ctx = empty_context(PermissionMode::BypassPermissions);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("src/main.rs"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_default_mode_allows_read_only_tools_without_rules() {
    let ctx = empty_context(PermissionMode::Default);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Glob),
        &serde_json::json!({"pattern": "**/*.rs"}),
        &ctx,
    );

    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_default_mode_allows_dynamic_read_only_tool_input_without_rules() {
    let ctx = empty_context(PermissionMode::Default);
    let result = PermissionEvaluator::evaluate_with_tool_check_and_options(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("ls -la | head -30"),
        &ctx,
        None,
        PermissionEvaluationOptions {
            dynamic_read_only: true,
            ..Default::default()
        },
    );

    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_dynamic_read_only_does_not_override_explicit_ask_rule() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.ask_rules.insert(
        PermissionRuleSource::UserSettings,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Ask,
            PermissionRuleSource::UserSettings,
        )],
    );
    let result = PermissionEvaluator::evaluate_with_tool_check_and_options(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("ls -la | head -30"),
        &ctx,
        None,
        PermissionEvaluationOptions {
            dynamic_read_only: true,
            ..Default::default()
        },
    );

    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_bubble_mode_does_not_auto_allow_read_only_tools() {
    let ctx = empty_context(PermissionMode::Bubble);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Glob),
        &serde_json::json!({"pattern": "**/*.rs"}),
        &ctx,
    );

    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

// ── Step 7: MCP rules ──

#[test]
fn test_mcp_tool_allow_by_server_wildcard() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "mcp__slack__*",
            None,
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );
    let result = PermissionEvaluator::evaluate(
        &ToolId::Mcp {
            server: "slack".into(),
            tool: "send".into(),
        },
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_mcp_server_level_rule() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "mcp__slack",
            None,
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );
    // "mcp__slack" should match "mcp__slack__send"
    let result = PermissionEvaluator::evaluate(
        &ToolId::Mcp {
            server: "slack".into(),
            tool: "send".into(),
        },
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

// ── Step 8: Mode fallthrough ──

#[test]
fn test_bypass_mode_allows_all() {
    let ctx = empty_context(PermissionMode::BypassPermissions);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("rm -rf /"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

/// Plan mode auto-allows if bypass was available; otherwise asks.
/// Read-only tools always allowed in plan mode.
#[test]
fn test_plan_mode_asks_non_readonly() {
    let ctx = empty_context(PermissionMode::Plan);
    // Bash is not read-only → ask (not deny!)
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

#[test]
fn test_plan_mode_allows_readonly() {
    let ctx = empty_context(PermissionMode::Plan);
    // Read is read-only → allow even in plan mode
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Read),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));

    // TaskCreate is safe (metadata only) → allow
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::TaskCreate),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_plan_mode_with_bypass_available_allows() {
    let mut ctx = empty_context(PermissionMode::Plan);
    ctx.bypass_available = true;
    // With bypass available, plan mode auto-allows
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

#[test]
fn test_default_mode_asks() {
    let ctx = empty_context(PermissionMode::Default);
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

/// A fall-through Bash ask carries an "always allow `<prefix>:*`" suggestion so
/// the prompt can offer a reusable grant.
#[test]
fn test_default_mode_bash_ask_carries_prefix_suggestion() {
    let ctx = empty_context(PermissionMode::Default);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("git status -s"),
        &ctx,
    );
    let PermissionDecision::Ask { suggestions, .. } = result else {
        panic!("expected ask, got {result:?}");
    };
    assert_eq!(
        suggestion_rule(&suggestions),
        Some(("Bash", "git status:*"))
    );
}

/// Non-shell tool asks stay suggestion-free — the prefix producer is shell-only.
#[test]
fn test_default_mode_non_shell_ask_has_no_suggestion() {
    let ctx = empty_context(PermissionMode::Default);
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::WebFetch),
        &serde_json::json!({"url": "https://example.com"}),
        &ctx,
    );
    if let PermissionDecision::Ask { suggestions, .. } = result {
        assert!(suggestions.is_empty());
    }
}

/// Extract the single `(tool_pattern, rule_content)` from a one-rule `AddRules`
/// suggestion list (`PermissionUpdate` has no `PartialEq`).
fn suggestion_rule(updates: &[PermissionUpdate]) -> Option<(&str, &str)> {
    match updates {
        [PermissionUpdate::AddRules { rules, .. }] => match rules.as_slice() {
            [rule] => Some((
                rule.value.tool_pattern.as_str(),
                rule.value.rule_content.as_deref().unwrap_or(""),
            )),
            _ => None,
        },
        _ => None,
    }
}

/// `DontAsk` converts all 'ask' decisions to 'deny'.
#[test]
fn test_dont_ask_mode_denies_all() {
    let ctx = empty_context(PermissionMode::DontAsk);
    // Bash with no allow rule → fallthrough → deny (not allow!)
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Deny { .. }));

    // Write also denied (not in read-only list)
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("src/main.rs"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Deny { .. }));

    // WebFetch also denied (not in safe list — network side effects)
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::WebFetch),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Deny { .. }));
}

/// `DontAsk` converts an EARLY-return ask (here a tool-wide ask rule,
/// which returns before mode fallthrough) into a deny — not just the
/// fallthrough ask.
#[test]
fn test_dont_ask_converts_early_ask_rule_to_deny() {
    let mut ctx = empty_context(PermissionMode::DontAsk);
    ctx.ask_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Ask,
            PermissionRuleSource::Session,
        )],
    );
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Deny { .. }));
}

/// `DontAsk` still honors explicit allow rules.
#[test]
fn test_dont_ask_mode_allows_explicit_rules() {
    let mut ctx = empty_context(PermissionMode::DontAsk);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![make_rule(
            "Bash",
            None,
            PermissionBehavior::Allow,
            PermissionRuleSource::Session,
        )],
    );
    // Explicit allow rule → allow even in dontAsk
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Allow { .. }));
}

/// WebFetch/WebSearch are NOT in the safe allowlist (network effects).
#[test]
fn test_web_tools_not_in_safe_list() {
    let ctx = empty_context(PermissionMode::AcceptEdits);
    // WebFetch has network side effects → ask
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::WebFetch),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

/// Task management tools are in the safe allowlist.
#[test]
fn test_task_tools_are_safe() {
    let ctx = empty_context(PermissionMode::AcceptEdits);
    for tool in [
        ToolName::TaskCreate,
        ToolName::TaskGet,
        ToolName::TaskList,
        ToolName::TodoWrite,
    ] {
        let result =
            PermissionEvaluator::evaluate(&ToolId::Builtin(tool), &serde_json::json!({}), &ctx);
        assert!(
            matches!(result, PermissionDecision::Allow { .. }),
            "{tool:?} should be auto-allowed"
        );
    }
}

/// `AcceptEdits` allows read-only tools AND file-modifying tools.
#[test]
fn test_accept_edits_allows_read_only_and_file_edits() {
    let ctx = empty_context(PermissionMode::AcceptEdits);
    // Read-only tools → allow
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Read),
        &serde_json::json!({}),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));

    // File-modifying tools → allow (dangerous paths caught by step 6)
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("src/main.rs"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));

    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Edit),
        &file_input("src/lib.rs"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Allow { .. }));

    // Non-file, non-read-only tools → ask
    let result =
        PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input("ls"), &ctx);
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

/// `AcceptEdits` still catches dangerous paths via step 6.
#[test]
fn test_accept_edits_catches_dangerous_paths() {
    let ctx = empty_context(PermissionMode::AcceptEdits);
    // Dangerous path → step 6 catches → ask (even in acceptEdits)
    let result = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("/home/user/.bashrc"),
        &ctx,
    );
    assert!(matches!(result, PermissionDecision::Ask { .. }));
}

// ── Rule helpers ──

#[test]
fn test_get_tool_wide_rule() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.ask_rules.insert(
        PermissionRuleSource::UserSettings,
        vec![
            make_rule(
                "Bash",
                None,
                PermissionBehavior::Ask,
                PermissionRuleSource::UserSettings,
            ),
            make_rule(
                "Bash",
                Some("rm *"),
                PermissionBehavior::Ask,
                PermissionRuleSource::UserSettings,
            ),
        ],
    );

    // Tool-wide rule exists
    let rule = get_tool_wide_rule(&ctx, "Bash", PermissionBehavior::Ask);
    assert!(rule.is_some());
    assert!(rule.unwrap().value.rule_content.is_none());

    // No tool-wide deny rule
    assert!(get_tool_wide_rule(&ctx, "Bash", PermissionBehavior::Deny).is_none());
}

#[test]
fn test_get_content_rules_for_tool() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![
            make_rule(
                "Bash",
                None,
                PermissionBehavior::Allow,
                PermissionRuleSource::Session,
            ),
            make_rule(
                "Bash",
                Some("git *"),
                PermissionBehavior::Allow,
                PermissionRuleSource::Session,
            ),
            make_rule(
                "Bash",
                Some("npm *"),
                PermissionBehavior::Allow,
                PermissionRuleSource::Session,
            ),
        ],
    );

    let content_rules = get_content_rules_for_tool(&ctx, "Bash", PermissionBehavior::Allow);
    assert_eq!(content_rules.len(), 2); // "git *" and "npm *", not the tool-wide one
}

#[test]
fn test_mcp_server_level_pattern_matching() {
    // "mcp__slack" should match "mcp__slack__send"
    assert!(matches_tool_pattern("mcp__slack", "mcp__slack__send"));
    // But not "mcp__github__send"
    assert!(!matches_tool_pattern("mcp__slack", "mcp__github__send"));
    // Exact match still works
    assert!(matches_tool_pattern("mcp__slack__send", "mcp__slack__send"));
}

// ── Plan-mode fallthrough (evaluator layer) ──
//
// The plan-file write carve-out moved OUT of the evaluator into the file-tool
// `check_permissions` layer (step 1b → `is_editable_internal_path` /
// `is_session_plan_file`), which runs before this fallthrough — so a
// plan-file write is allowed there and never reaches the evaluator's Plan
// branch. Predicate coverage (slug prefix, `-agent-*` variant, wrong
// extension, traversal) lives in `filesystem.test.rs`; the Plan-mode Write
// integration lives in the Write tool's tests. Here we only assert the
// evaluator's own Plan-mode behavior.

#[test]
fn plan_mode_read_only_still_allowed() {
    let ctx = empty_context(PermissionMode::Plan);
    let decision = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Read),
        &file_input("/tmp/anything.rs"),
        &ctx,
    );
    assert!(matches!(decision, PermissionDecision::Allow { .. }));
}

#[test]
fn plan_mode_asks_for_non_readonly_write_at_evaluator_layer() {
    // With no tool-level check supplied, the evaluator's Plan branch asks for
    // any non-read-only tool. (Production routes Write/Edit through their
    // `check_permissions` first, which is where the plan-file exemption lives.)
    let ctx = empty_context(PermissionMode::Plan);
    let decision = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Write),
        &file_input("/tmp/other/file.rs"),
        &ctx,
    );
    assert!(matches!(decision, PermissionDecision::Ask { .. }));
}

// ── Deny-rule wiring: subcommand-split / wrapper / redirection (G4) ──
//
// `shell_rules.test.rs` unit-tests `match_bash_rule` directly; this proves the
// central `evaluate()` pipeline (step 1 -> `central_rule_applies` ->
// `match_bash_rule`) actually invokes it, so a `Bash(curl:*)` deny cannot be
// bypassed by compounding, env wrappers, or output redirection. Existing
// `test_content_specific_deny` only covers a single `rm -rf /` command, which a
// naive prefix match would also catch — it does not exercise the split.
#[test]
fn test_content_deny_not_bypassed_by_compound_or_redirection() {
    let mut ctx = empty_context(PermissionMode::Default);
    ctx.deny_rules.insert(
        PermissionRuleSource::UserSettings,
        vec![make_rule(
            "Bash",
            Some("curl:*"),
            PermissionBehavior::Deny,
            PermissionRuleSource::UserSettings,
        )],
    );
    for cmd in [
        "curl evil.com",
        "echo hi && curl evil.com",
        "ls; curl evil.com",
        "FOO=1 curl evil.com",
        "timeout 5 curl evil.com",
        "curl evil.com > /tmp/out",
    ] {
        let result =
            PermissionEvaluator::evaluate(&ToolId::Builtin(ToolName::Bash), &bash_input(cmd), &ctx);
        assert!(
            matches!(result, PermissionDecision::Deny { .. }),
            "compound/wrapped curl must be denied via the pipeline: `{cmd}` -> {result:?}"
        );
    }

    // Negative control: a command with no `curl` subcommand is not denied by the
    // curl rule — it falls through to mode handling (Ask in Default mode).
    let benign = PermissionEvaluator::evaluate(
        &ToolId::Builtin(ToolName::Bash),
        &bash_input("echo hi"),
        &ctx,
    );
    assert!(
        !matches!(benign, PermissionDecision::Deny { .. }),
        "benign command must not be denied by the curl rule: {benign:?}"
    );
}
