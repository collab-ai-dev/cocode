use pretty_assertions::assert_eq;
use serde_json::json;

fn prompt(
    tool_name: &str,
    original_input: Option<serde_json::Value>,
    suggestions: Vec<coco_types::PermissionUpdate>,
) -> crate::state::PermissionPromptState {
    crate::state::PermissionPromptState {
        request_id: "req-1".to_string(),
        tool_name: tool_name.to_string(),
        description: "Allow this operation?".to_string(),
        detail: crate::state::PermissionDetail::Generic {
            input_preview: tool_name.to_string(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input,
        cwd: None,
        permission_suggestions: suggestions,
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

fn prompt_with_cwd(
    tool_name: &str,
    original_input: Option<serde_json::Value>,
    cwd: &str,
) -> crate::state::PermissionPromptState {
    let mut p = prompt(tool_name, original_input, vec![]);
    p.cwd = Some(cwd.to_string());
    p
}

fn allow_rule(
    source: coco_types::PermissionRuleSource,
    tool_pattern: &str,
    rule_content: &str,
) -> coco_types::PermissionRule {
    coco_types::PermissionRule {
        source,
        behavior: coco_types::PermissionBehavior::Allow,
        value: coco_types::PermissionRuleValue {
            tool_pattern: tool_pattern.to_string(),
            rule_content: Some(rule_content.to_string()),
        },
    }
}

fn rule_summaries(update: &coco_types::PermissionUpdate) -> Vec<(String, Option<String>)> {
    match update {
        coco_types::PermissionUpdate::AddRules { rules, .. } => rules
            .iter()
            .map(|r| (r.value.tool_pattern.clone(), r.value.rule_content.clone()))
            .collect(),
        other => panic!("expected AddRules, got {other:?}"),
    }
}

#[test]
fn session_allow_auto_filters_accept_edits_mode_update() {
    let p = prompt(
        "Write",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }],
    );

    let updates =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Auto);

    assert!(updates.is_empty(), "Auto must not downgrade to AcceptEdits");
}

#[test]
fn session_allow_accept_edits_filters_accept_edits_mode_update() {
    let p = prompt(
        "Write",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }],
    );

    let updates = crate::permission_options::session_allow_updates(
        &p,
        coco_types::PermissionMode::AcceptEdits,
    );

    assert!(
        updates.is_empty(),
        "AcceptEdits must not generate SetMode(AcceptEdits)"
    );
}

#[test]
fn session_allow_default_keeps_core_accept_edits_suggestion() {
    let p = prompt(
        "Write",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }],
    );

    let updates =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);

    assert!(
        matches!(
            updates.as_slice(),
            [coco_types::PermissionUpdate::SetMode {
                mode: coco_types::PermissionMode::AcceptEdits,
            }]
        ),
        "Default mode may accept core's session mode suggestion: {updates:?}"
    );
}

#[test]
fn session_allow_plan_keeps_core_accept_edits_suggestion() {
    let p = prompt(
        "Write",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }],
    );

    let updates =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Plan);

    assert!(matches!(
        updates.as_slice(),
        [coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }]
    ));
}

#[test]
fn add_directories_suggestion_offers_session_and_local() {
    // An out-of-dir write Ask carries an AddDirectories suggestion. Both the
    // session and local rows mirror it (to their respective destinations) so the
    // dialog offers 4 rows — adding the working dir is the only update that
    // unblocks the path gate.
    let p = prompt(
        "Edit",
        None,
        vec![coco_types::PermissionUpdate::AddDirectories {
            directories: vec!["/tmp/project".into()],
            destination: coco_types::PermissionUpdateDestination::Session,
        }],
    );

    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    let local = crate::permission_options::local_allow_updates(&p);

    assert!(matches!(
        session.as_slice(),
        [coco_types::PermissionUpdate::AddDirectories { directories, destination }]
            if directories == &vec!["/tmp/project".to_string()]
                && *destination == coco_types::PermissionUpdateDestination::Session
    ));
    assert!(matches!(
        local.as_slice(),
        [coco_types::PermissionUpdate::AddDirectories { directories, destination }]
            if directories == &vec!["/tmp/project".to_string()]
                && *destination == coco_types::PermissionUpdateDestination::LocalSettings
    ));
}

#[test]
fn local_allow_write_tool_scopes_to_directory_without_mode_update() {
    let p = prompt(
        "Write",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![coco_types::PermissionUpdate::SetMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        }],
    );

    let updates = crate::permission_options::local_allow_updates(&p);

    assert_eq!(updates.len(), 1);
    let coco_types::PermissionUpdate::AddRules { rules, destination } = &updates[0] else {
        panic!("expected AddRules update")
    };
    assert_eq!(
        *destination,
        coco_types::PermissionUpdateDestination::LocalSettings
    );
    // Every derived rule is a LocalSettings Edit grant scoped to the `proj`
    // directory with the `//abs/**` convention. A symlinked `/tmp` (macOS
    // canonicalizes to `/private/tmp`) adds a second resolved form — matching
    // the engine's `get_paths_for_permission_check` coverage — so assert the
    // property, not one platform-specific path.
    assert!(!rules.is_empty());
    assert!(rules.iter().all(|rule| {
        rule.source == coco_types::PermissionRuleSource::LocalSettings
            && rule.value.tool_pattern == "Edit"
            && rule
                .value
                .rule_content
                .as_deref()
                .is_some_and(|c| c.starts_with("//") && c.ends_with("/proj/**"))
    }));
}

#[test]
fn local_allow_apply_patch_scopes_to_patch_target_dirs() {
    let patch = "*** Begin Patch\n\
                 *** Add File: /tmp/plans/calm-bouncing-biscuit.md\n\
                 +# plan\n\
                 *** Update File: /tmp/proj/src/main.rs\n\
                 @@\n\
                 -a\n\
                 +b\n\
                 *** End Patch\n";
    let p = prompt("apply_patch", Some(json!({ "patch": patch })), vec![]);

    let updates = crate::permission_options::local_allow_updates(&p);

    assert_eq!(updates.len(), 1);
    let rules = rule_summaries(&updates[0]);
    // Both patch targets are scoped to their parent directory as Edit grants
    // (`plans/` for the add, `proj/src/` for the update). Symlinked `/tmp` may
    // contribute extra resolved forms, so assert each target dir is covered
    // rather than pinning the exact list.
    assert!(rules.iter().all(|(tool, content)| {
        tool == "Edit"
            && content
                .as_deref()
                .is_some_and(|c| c.starts_with("//") && c.ends_with("/**"))
    }));
    assert!(
        rules
            .iter()
            .any(|(_, c)| c.as_deref().is_some_and(|c| c.ends_with("/plans/**")))
    );
    assert!(
        rules
            .iter()
            .any(|(_, c)| c.as_deref().is_some_and(|c| c.ends_with("/proj/src/**")))
    );
}

#[test]
fn read_tool_session_and_local_use_matching_directory_rules() {
    let p = prompt(
        "Read",
        Some(json!({"file_path": "/tmp/proj/notes.md"})),
        vec![],
    );

    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    let local = crate::permission_options::local_allow_updates(&p);

    let [
        coco_types::PermissionUpdate::AddRules {
            rules: session_rules,
            destination: session_destination,
        },
    ] = session.as_slice()
    else {
        panic!("expected session AddRules")
    };
    let [
        coco_types::PermissionUpdate::AddRules {
            rules: local_rules,
            destination: local_destination,
        },
    ] = local.as_slice()
    else {
        panic!("expected local AddRules")
    };
    assert_eq!(
        *session_destination,
        coco_types::PermissionUpdateDestination::Session
    );
    assert_eq!(
        *local_destination,
        coco_types::PermissionUpdateDestination::LocalSettings
    );
    assert_eq!(
        session_rules[0].source,
        coco_types::PermissionRuleSource::Session
    );
    assert_eq!(
        local_rules[0].source,
        coco_types::PermissionRuleSource::LocalSettings
    );
    assert_eq!(
        session_rules[0].value.rule_content,
        local_rules[0].value.rule_content
    );
}

#[test]
fn shell_suggestions_are_converted_to_session_or_local_destination() {
    let p = prompt(
        "Bash",
        Some(json!({"command": "npm test"})),
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![allow_rule(
                coco_types::PermissionRuleSource::LocalSettings,
                "Bash",
                "npm:*",
            )],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
    );

    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    let local = crate::permission_options::local_allow_updates(&p);

    let [
        coco_types::PermissionUpdate::AddRules {
            rules: session_rules,
            destination: session_destination,
        },
    ] = session.as_slice()
    else {
        panic!("expected session AddRules")
    };
    let [
        coco_types::PermissionUpdate::AddRules {
            rules: local_rules,
            destination: local_destination,
        },
    ] = local.as_slice()
    else {
        panic!("expected local AddRules")
    };
    assert_eq!(
        *session_destination,
        coco_types::PermissionUpdateDestination::Session
    );
    assert_eq!(
        *local_destination,
        coco_types::PermissionUpdateDestination::LocalSettings
    );
    assert_eq!(
        session_rules[0].source,
        coco_types::PermissionRuleSource::Session
    );
    assert_eq!(
        local_rules[0].source,
        coco_types::PermissionRuleSource::LocalSettings
    );
    assert_eq!(
        session_rules[0].value.rule_content.as_deref(),
        Some("npm:*")
    );
    assert_eq!(local_rules[0].value.rule_content.as_deref(), Some("npm:*"));
}

#[test]
fn shell_exact_suggestions_are_converted_to_session_or_local_destination() {
    let p = prompt(
        "Bash",
        Some(json!({"command": "git status"})),
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![allow_rule(
                coco_types::PermissionRuleSource::LocalSettings,
                "Bash",
                "git status",
            )],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
    );

    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    let local = crate::permission_options::local_allow_updates(&p);

    assert_eq!(
        rule_summaries(&session[0]),
        vec![("Bash".to_string(), Some("git status".to_string()))]
    );
    assert_eq!(
        rule_summaries(&local[0]),
        vec![("Bash".to_string(), Some("git status".to_string()))]
    );
}

#[test]
fn shell_wildcard_suggestions_are_rejected_for_session_and_local() {
    for rule_content in ["*", "git *", "docker * --read-only"] {
        let p = prompt(
            "Bash",
            Some(json!({"command": "git status"})),
            vec![coco_types::PermissionUpdate::AddRules {
                rules: vec![allow_rule(
                    coco_types::PermissionRuleSource::LocalSettings,
                    "Bash",
                    rule_content,
                )],
                destination: coco_types::PermissionUpdateDestination::LocalSettings,
            }],
        );

        assert!(
            crate::permission_options::session_allow_updates(
                &p,
                coco_types::PermissionMode::Default
            )
            .is_empty(),
            "{rule_content:?} must not produce a session grant"
        );
        assert!(
            crate::permission_options::local_allow_updates(&p).is_empty(),
            "{rule_content:?} must not produce a local grant"
        );
    }
}

#[test]
fn powershell_exact_and_prefix_pass_but_wildcards_are_rejected() {
    let exact = prompt(
        "PowerShell",
        Some(json!({"command": "Get-ChildItem"})),
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![allow_rule(
                coco_types::PermissionRuleSource::LocalSettings,
                "PowerShell",
                "Get-ChildItem",
            )],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
    );
    let prefix = prompt(
        "PowerShell",
        Some(json!({"command": "Get-ChildItem C:\\"})),
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![allow_rule(
                coco_types::PermissionRuleSource::LocalSettings,
                "PowerShell",
                "Get-ChildItem ",
            )],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
    );
    let wildcard = prompt(
        "PowerShell",
        Some(json!({"command": "Get-ChildItem C:\\"})),
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![allow_rule(
                coco_types::PermissionRuleSource::LocalSettings,
                "PowerShell",
                "Get-*",
            )],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
    );

    assert!(!crate::permission_options::local_allow_updates(&exact).is_empty());
    assert!(!crate::permission_options::local_allow_updates(&prefix).is_empty());
    assert!(crate::permission_options::local_allow_updates(&wildcard).is_empty());
    assert!(
        crate::permission_options::session_allow_updates(
            &wildcard,
            coco_types::PermissionMode::Default
        )
        .is_empty()
    );
}

#[test]
fn relative_read_path_resolves_against_request_cwd() {
    let p = prompt_with_cwd(
        "Read",
        Some(json!({"file_path": "src/main.rs"})),
        "/workspace/project",
    );

    let updates =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);

    assert_eq!(
        rule_summaries(&updates[0]),
        vec![(
            "Read".to_string(),
            Some("//workspace/project/src/**".to_string())
        )]
    );
}

#[test]
fn relative_edit_path_resolves_against_request_cwd() {
    let p = prompt_with_cwd(
        "Write",
        Some(json!({"file_path": "notes/todo.md"})),
        "/workspace/project",
    );

    let updates = crate::permission_options::local_allow_updates(&p);

    assert_eq!(
        rule_summaries(&updates[0]),
        vec![(
            "Edit".to_string(),
            Some("//workspace/project/notes/**".to_string())
        )]
    );
}

#[test]
fn relative_paths_without_cwd_do_not_infer_scoped_allow() {
    let read = prompt("Read", Some(json!({"file_path": "src/main.rs"})), vec![]);
    let write = prompt("Write", Some(json!({"file_path": "src/main.rs"})), vec![]);

    assert!(
        crate::permission_options::session_allow_updates(
            &read,
            coco_types::PermissionMode::Default
        )
        .is_empty()
    );
    assert!(crate::permission_options::local_allow_updates(&read).is_empty());
    assert!(
        crate::permission_options::session_allow_updates(
            &write,
            coco_types::PermissionMode::Default
        )
        .is_empty()
    );
    assert!(crate::permission_options::local_allow_updates(&write).is_empty());
}

#[test]
fn underivable_mutating_tool_has_no_scoped_allow_options() {
    let p = prompt("apply_patch", Some(json!({"patch": "garbage"})), vec![]);

    assert!(
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default)
            .is_empty()
    );
    assert!(crate::permission_options::local_allow_updates(&p).is_empty());
    assert_eq!(
        crate::permission_options::classic_actions(&p, coco_types::PermissionMode::Default),
        vec![
            crate::permission_options::PermissionAction::ApproveOnce,
            crate::permission_options::PermissionAction::Deny,
        ]
    );
}

#[test]
fn edit_path_allow_update_none_for_underivable_write_input() {
    assert!(
        crate::permission_options::edit_path_allow_update(
            "apply_patch",
            Some(&json!({"patch": "garbage"})),
            None,
            coco_types::PermissionRuleSource::LocalSettings,
            coco_types::PermissionUpdateDestination::LocalSettings,
        )
        .is_none()
    );
    assert!(
        crate::permission_options::edit_path_allow_update(
            "Write",
            Some(&json!({})),
            None,
            coco_types::PermissionRuleSource::LocalSettings,
            coco_types::PermissionUpdateDestination::LocalSettings,
        )
        .is_none()
    );
    assert!(
        crate::permission_options::edit_path_allow_update(
            "Write",
            None,
            None,
            coco_types::PermissionRuleSource::LocalSettings,
            coco_types::PermissionUpdateDestination::LocalSettings,
        )
        .is_none()
    );
}

#[test]
fn mcp_tool_without_suggestions_offers_exact_name_always_allow() {
    // MCP tools reach the prompt with no engine suggestion and no derivable
    // path. They have no narrower scope, so both the session and local rows
    // fall back to an exact-tool-name allow, restoring the "don't ask again"
    // affordance the refactor had dropped.
    let p = prompt(
        "mcp__github__search_issues",
        Some(json!({"q": "bug"})),
        vec![],
    );

    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    let local = crate::permission_options::local_allow_updates(&p);

    let assert_tool_wide = |updates: &[coco_types::PermissionUpdate], dest| {
        assert!(matches!(
            updates,
            [coco_types::PermissionUpdate::AddRules { rules, destination }]
                if *destination == dest
                    && rules.len() == 1
                    && rules[0].value.tool_pattern == "mcp__github__search_issues"
                    && rules[0].value.rule_content.is_none()
        ));
    };
    assert_tool_wide(&session, coco_types::PermissionUpdateDestination::Session);
    assert_tool_wide(
        &local,
        coco_types::PermissionUpdateDestination::LocalSettings,
    );

    assert_eq!(
        crate::permission_options::classic_actions(&p, coco_types::PermissionMode::Default),
        vec![
            crate::permission_options::PermissionAction::ApproveOnce,
            crate::permission_options::PermissionAction::AllowSession,
            crate::permission_options::PermissionAction::AllowLocal,
            crate::permission_options::PermissionAction::Deny,
        ]
    );
}

#[test]
fn shell_tool_without_command_rule_has_no_tool_wide_fallback() {
    // A tool-WIDE Bash allow would approve any command — the fallback must
    // never fire for shell/file tools, only scoped command/path rules.
    let p = prompt("Bash", Some(json!({"command": "ls"})), vec![]);

    assert!(
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default)
            .is_empty()
    );
    assert!(crate::permission_options::local_allow_updates(&p).is_empty());
}

// ── Editable always-allow prefix (shell tools) ──

/// A Bash prompt with the editable prefix seeded from `command`, with the
/// classic action row at `selected` focused (1 = AllowSession, 2 = AllowLocal).
fn shell_prompt(command: &str, selected: usize) -> crate::state::PermissionPromptState {
    let mut p = prompt("Bash", Some(json!({ "command": command })), vec![]);
    p.prefix_input = Some(crate::state::PrefixInputState::new(
        coco_permissions::shell_rules::editable_prefix_default(command),
    ));
    p.selected_choice = selected;
    p
}

#[test]
fn prefix_seed_two_word_then_first_word_then_exact() {
    assert_eq!(
        shell_prompt("git status -s", 0).prefix_input.unwrap().value,
        "git status:*"
    );
    // No subcommand-shaped token → single-word fallback.
    assert_eq!(
        shell_prompt("ls -la", 0).prefix_input.unwrap().value,
        "ls:*"
    );
    // Bare shell blocked in both extractors → exact command.
    assert_eq!(
        shell_prompt("bash -c 'x'", 0).prefix_input.unwrap().value,
        "bash -c 'x'"
    );
}

#[test]
fn prefix_editing_only_on_allow_rows() {
    let mode = coco_types::PermissionMode::Default;
    // actions: [ApproveOnce, AllowSession, AllowLocal, Deny]
    assert!(!crate::permission_options::prefix_editing(
        &shell_prompt("git status -s", 0),
        mode
    ));
    assert!(crate::permission_options::prefix_editing(
        &shell_prompt("git status -s", 1),
        mode
    ));
    assert!(crate::permission_options::prefix_editing(
        &shell_prompt("git status -s", 2),
        mode
    ));
    assert!(!crate::permission_options::prefix_editing(
        &shell_prompt("git status -s", 3),
        mode
    ));
}

#[test]
fn shell_prompt_offers_both_allow_rows() {
    use crate::permission_options::PermissionAction::*;
    let actions = crate::permission_options::classic_actions(
        &shell_prompt("git status -s", 0),
        coco_types::PermissionMode::Default,
    );
    assert_eq!(actions, vec![ApproveOnce, AllowSession, AllowLocal, Deny]);
}

#[test]
fn edited_prefix_drives_both_destinations() {
    let mut p = shell_prompt("git status -s", 2);
    // User narrows the seeded "git status:*" down to "git:*".
    p.prefix_input = Some(crate::state::PrefixInputState::new("git:*".to_string()));

    let local = crate::permission_options::local_allow_updates(&p);
    assert_eq!(
        rule_summaries(&local[0]),
        vec![("Bash".to_string(), Some("git:*".to_string()))]
    );
    let session =
        crate::permission_options::session_allow_updates(&p, coco_types::PermissionMode::Default);
    assert_eq!(
        rule_summaries(&session[0]),
        vec![("Bash".to_string(), Some("git:*".to_string()))]
    );
}

#[test]
fn empty_or_unsafe_edited_prefix_yields_no_rule() {
    // Empty → allow once (no rule).
    let mut p = shell_prompt("git status -s", 2);
    p.prefix_input = Some(crate::state::PrefixInputState::new(String::new()));
    assert!(crate::permission_options::local_allow_updates(&p).is_empty());

    // Wildcard isn't a safe scoped allow (only Exact/Prefix accepted).
    p.prefix_input = Some(crate::state::PrefixInputState::new("git *".to_string()));
    assert!(crate::permission_options::local_allow_updates(&p).is_empty());
}

#[test]
fn prefix_input_edit_ops() {
    let mut input = crate::state::PrefixInputState::new("git status:*".to_string());
    assert_eq!(input.cursor, "git status:*".len());
    input.home();
    assert_eq!(input.cursor, 0);
    input.end();
    input.backspace();
    input.backspace();
    assert_eq!(input.value, "git status");
    input.delete_word_backward();
    assert_eq!(input.value, "git ");
    input.insert('x');
    assert_eq!(input.value, "git x");
    input.left();
    input.insert('_');
    assert_eq!(input.value, "git _x");
}
