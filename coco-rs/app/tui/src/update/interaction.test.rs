//! Interaction precedence and bottom-pane prompt tests.

use super::*;
use crate::command::UserCommand;
use crate::state::AppState;
use crate::state::MemoryDialogEntry;
use crate::state::MemoryDialogRowKind;
use crate::state::MemoryDialogScope;
use crate::state::MemoryDialogState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::ui::ToastSeverity;
use tokio::sync::mpsc;

#[tokio::test]
async fn confirm_memory_dialog_sends_open_memory_file_command() {
    let path = std::path::PathBuf::from("/tmp/coco-memory-test/CLAUDE.md");
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::MemoryDialog(MemoryDialogState {
            entries: vec![MemoryDialogEntry {
                path: path.clone(),
                label: "Project memory".to_string(),
                scope: MemoryDialogScope::Project,
                row_kind: MemoryDialogRowKind::File {
                    exists: false,
                    read_only: false,
                },
            }],
            selected: 0,
        }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    confirm(&mut state, &tx).await;

    let UserCommand::OpenMemoryFile { path: sent_path } =
        rx.try_recv().expect("memory open command sent")
    else {
        panic!("expected OpenMemoryFile")
    };
    assert_eq!(sent_path, path);
    assert!(
        !state.ui.has_active_surface(),
        "state dismissed after select"
    );
}

#[tokio::test]
async fn confirm_memory_dialog_keeps_non_file_rows_open() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::MemoryDialog(MemoryDialogState {
            entries: vec![MemoryDialogEntry {
                path: std::path::PathBuf::from("/tmp/coco-memory-test"),
                label: "Auto-memory folder".to_string(),
                scope: MemoryDialogScope::User,
                row_kind: MemoryDialogRowKind::Folder { enabled: true },
            }],
            selected: 0,
        }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    confirm(&mut state, &tx).await;

    assert!(rx.try_recv().is_err(), "no editor command for non-file row");
    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::MemoryDialog(_))),
        "state stays open"
    );
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Warning);
}

// ── Permission state: multi-choice commit path ──
//
// The user picks via arrows and Enter; the chosen `value` is sent back
// as typed `resolution_detail` on `ApprovalResponse`.

fn permission_with_choices(values: &[&str], selected: usize) -> AppState {
    permission_with_choices_for_tool("ExitPlanMode", values, selected)
}

fn permission_with_choices_for_tool(tool_name: &str, values: &[&str], selected: usize) -> AppState {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    use coco_types::PermissionAskChoice;

    let mut s = AppState::new();
    let choices: Vec<PermissionAskChoice> = values
        .iter()
        .map(|v| PermissionAskChoice {
            value: (*v).to_string(),
            label: (*v).to_string(),
            description: None,
        })
        .collect();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: tool_name.into(),
        description: "Exit plan mode?".into(),
        detail: PermissionDetail::Generic {
            input_preview: String::new(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: Some(choices),
        selected_choice: selected,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input: Some(serde_json::json!({"plan": "do the thing"})),
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    s
}

fn permission_with_allowed_prompts(values: &[&str], selected: usize) -> AppState {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    use coco_types::PermissionAskChoice;

    let mut s = AppState::new();
    let choices: Vec<PermissionAskChoice> = values
        .iter()
        .map(|v| PermissionAskChoice {
            value: (*v).to_string(),
            label: (*v).to_string(),
            description: None,
        })
        .collect();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
        description: "Exit plan mode?".into(),
        detail: PermissionDetail::ExitPlanMode {
            outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
            plan: Some("# Plan".into()),
            plan_file_path: Some("/tmp/plan.md".into()),
            allowed_prompts: vec![coco_types::ExitPlanModeAllowedPrompt {
                tool: "Bash".into(),
                prompt: "cargo test".into(),
            }],
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: Some(choices),
        selected_choice: selected,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input: Some(serde_json::json!({"plan": "do the thing"})),
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    s
}

#[tokio::test]
async fn confirm_with_choice_sends_exit_plan_resolution_detail() {
    // Selecting "yes-accept-edits-clear-context" should send approved=true
    // with a typed resolution detail — the engine reads this off
    // ToolUseContext to flag history clear.
    let mut s = permission_with_choices(
        &[
            "yes-accept-edits-keep-context",
            "yes-accept-edits-clear-context",
            "no",
        ],
        1, // "yes-accept-edits-clear-context"
    );
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let cmd = rx.try_recv().expect("approval sent");
    let UserCommand::ApprovalResponse {
        approved,
        updated_input,
        resolution_detail,
        ..
    } = cmd
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(approved, "non-'no' choice should approve");
    assert!(updated_input.is_none());
    assert!(matches!(
        resolution_detail,
        Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
            choice: coco_types::ExitPlanChoice::ClearAcceptEdits,
            edited_plan: None,
        })
    ));
    assert!(!s.ui.has_active_surface(), "state dismissed after commit");
}

#[tokio::test]
async fn confirm_with_allowed_prompts_sends_session_rules() {
    let mut s = permission_with_allowed_prompts(
        &[
            "yes-default-keep-context",
            "yes-accept-edits-clear-context",
            "no",
        ],
        0,
    );
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved,
        permission_updates,
        ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(approved);
    let [coco_types::PermissionUpdate::AddRules { rules, destination }] =
        permission_updates.as_slice()
    else {
        panic!("expected one AddRules update, got {permission_updates:?}")
    };
    assert_eq!(
        *destination,
        coco_types::PermissionUpdateDestination::Session
    );
    let [rule] = rules.as_slice() else {
        panic!("expected one rule, got {rules:?}")
    };
    assert_eq!(rule.source, coco_types::PermissionRuleSource::Session);
    assert_eq!(rule.behavior, coco_types::PermissionBehavior::Allow);
    assert_eq!(rule.value.tool_pattern, "Bash");
    assert_eq!(rule.value.rule_content.as_deref(), Some("cargo test"));
}

#[tokio::test]
async fn confirm_with_no_choice_sends_approved_false() {
    // "no" is the sentinel for deny; engine treats it as a regular
    // denial (tool doesn't execute). Typed detail still carries the
    // value so logs/audits see what the user picked.
    let mut s = permission_with_choices(
        &[
            "yes-accept-edits-keep-context",
            "yes-accept-edits-clear-context",
            "no",
        ],
        2, // "no"
    );
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let cmd = rx.try_recv().expect("approval sent");
    let UserCommand::ApprovalResponse {
        approved,
        updated_input,
        resolution_detail,
        feedback,
        permission_updates,
        ..
    } = cmd
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(!approved, "'no' choice should deny");
    assert!(permission_updates.is_empty());
    assert_eq!(
        feedback.as_deref(),
        Some("User rejected the plan. Stay in plan mode and continue planning.")
    );
    assert!(updated_input.is_none());
    assert!(matches!(
        resolution_detail,
        Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
            choice: coco_types::ExitPlanChoice::No,
            edited_plan: None,
        })
    ));
}

#[tokio::test]
async fn confirm_with_generic_no_choice_has_no_plan_feedback() {
    let mut s = permission_with_choices_for_tool("SomeTool", &["yes", "no"], 1);
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved, feedback, ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(!approved);
    assert_eq!(feedback, None);
}

#[tokio::test]
async fn approve_with_choice_takes_same_path_as_confirm() {
    // Pressing 'y' (Approve) when choices are present must commit the
    // currently-focused choice, not the implicit yes — otherwise the
    // tool would see updated_input=None and lose the user's pick.
    let mut s = permission_with_choices(
        &[
            "yes-accept-edits-keep-context",
            "yes-accept-edits-clear-context",
        ],
        1,
    );
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    approve(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved,
        updated_input,
        resolution_detail,
        ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!()
    };
    assert!(approved);
    assert!(updated_input.is_none());
    assert!(matches!(
        resolution_detail,
        Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
            choice: coco_types::ExitPlanChoice::ClearAcceptEdits,
            edited_plan: None,
        })
    ));
}

#[tokio::test]
async fn confirm_classic_yes_no_approves_selected_action() {
    // No choices → Enter commits the focused classic action.
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        description: "Run".into(),
        detail: PermissionDetail::Generic {
            input_preview: "ls".into(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("ls".into()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved,
        always_allow,
        permission_updates,
        ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(approved);
    assert!(!always_allow);
    assert!(permission_updates.is_empty());
    assert!(!s.ui.has_active_surface(), "state dismissed");
}

#[tokio::test]
async fn confirm_classic_always_allow_sends_session_update() {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        description: "Run".into(),
        detail: PermissionDetail::Generic {
            input_preview: "ls".into(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 1,
        display_input: coco_types::PermissionDisplayInput::Command("ls".into()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![coco_types::PermissionRule {
                source: coco_types::PermissionRuleSource::LocalSettings,
                behavior: coco_types::PermissionBehavior::Allow,
                value: coco_types::PermissionRuleValue {
                    tool_pattern: "Bash".to_string(),
                    rule_content: Some("ls".to_string()),
                },
            }],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved,
        always_allow,
        permission_updates,
        ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(approved);
    assert!(always_allow);
    assert_eq!(permission_updates.len(), 1);
    assert!(!s.ui.has_active_surface(), "state dismissed");
}

#[tokio::test]
async fn confirm_classic_read_always_allow_sends_path_scoped_local_update() {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    let dir = std::env::temp_dir().join("coco-tui-read-permission-test");
    let file = dir.join("notes.txt");
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Read".into(),
        description: "Read outside cwd".into(),
        detail: PermissionDetail::Generic {
            input_preview: file.display().to_string(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 2,
        display_input: coco_types::PermissionDisplayInput::Text(file.display().to_string()),
        original_input: Some(serde_json::json!({"file_path": file.to_string_lossy()})),
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        permission_updates, ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    let [coco_types::PermissionUpdate::AddRules { rules, destination }] =
        permission_updates.as_slice()
    else {
        panic!("expected AddRules update")
    };
    assert_eq!(
        *destination,
        coco_types::PermissionUpdateDestination::LocalSettings
    );
    assert_eq!(rules[0].value.tool_pattern, "Read");
    let expected = format!("/{}/**", dir.to_string_lossy());
    assert_eq!(
        rules[0].value.rule_content.as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn nav_advances_selected_choice_with_wraparound() {
    let mut s = permission_with_choices(&["a", "b", "c"], 0);
    nav(&mut s, 1);
    let Some(PanePromptState::Permission(p)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!()
    };
    assert_eq!(p.selected_choice, 1);
    nav(&mut s, 5);
    let Some(PanePromptState::Permission(p)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!()
    };
    assert_eq!(p.selected_choice, 0);
    nav(&mut s, -1);
    let Some(PanePromptState::Permission(p)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!()
    };
    assert_eq!(p.selected_choice, 2);
}

#[tokio::test]
async fn down_then_enter_commits_always_allow() {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        description: "Run".into(),
        detail: PermissionDetail::Generic {
            input_preview: "ls".into(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("ls".into()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![coco_types::PermissionRule {
                source: coco_types::PermissionRuleSource::LocalSettings,
                behavior: coco_types::PermissionBehavior::Allow,
                value: coco_types::PermissionRuleValue {
                    tool_pattern: "Bash".to_string(),
                    rule_content: Some("ls".to_string()),
                },
            }],
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    nav(&mut s, 1);

    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    confirm(&mut s, &tx).await;

    let UserCommand::ApprovalResponse {
        approved,
        always_allow,
        permission_updates,
        ..
    } = rx.try_recv().expect("approval sent")
    else {
        panic!("expected ApprovalResponse")
    };
    assert!(approved);
    assert!(always_allow);
    assert_eq!(permission_updates.len(), 1);
}

#[test]
fn build_choice_detail_for_exit_plan_choice() {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    use coco_types::PermissionAskChoice;

    let p = PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "ExitPlanMode".into(),
        description: String::new(),
        detail: PermissionDetail::Generic {
            input_preview: String::new(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: Some(vec![PermissionAskChoice {
            value: coco_types::ExitPlanChoice::KeepDefault.as_str().into(),
            label: "Keep context".into(),
            description: None,
        }]),
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input: Some(serde_json::json!({"existing": 42, "other": "v"})),
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    };
    assert!(matches!(
        crate::bottom_pane::permission::build_choice_detail(&p),
        Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
            choice: coco_types::ExitPlanChoice::KeepDefault,
            edited_plan: None,
        })
    ));
}

#[test]
fn build_choice_detail_none_when_cursor_out_of_range() {
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;

    let p = PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Foo".into(),
        description: String::new(),
        detail: PermissionDetail::Generic {
            input_preview: String::new(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: Some(vec![]),
        selected_choice: 5,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    };
    assert!(crate::bottom_pane::permission::build_choice_detail(&p).is_none());
}

// === AskUserQuestion key + answer mechanics ===

fn question_option(label: &str) -> crate::state::QuestionOption {
    crate::state::QuestionOption {
        label: label.into(),
        description: String::new(),
        preview: None,
    }
}

fn question_item(
    options: Vec<crate::state::QuestionOption>,
    multi_select: bool,
) -> crate::state::QuestionItem {
    crate::state::QuestionItem {
        header: "h".into(),
        question: "Q?".into(),
        options,
        multi_select,
        selected: None,
        checked: Vec::new(),
        other_input: crate::state::OtherInputState::default(),
    }
}

fn question_state(item: crate::state::QuestionItem) -> AppState {
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Question(
        crate::state::QuestionPromptState {
            request_id: "q1".into(),
            original_input: serde_json::json!({}),
            questions: vec![item],
            current_question: crate::state::QuestionPage::Question(0),
            focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
            is_in_plan_mode: false,
        },
    ));
    s
}

fn focused_selected(s: &AppState) -> Option<usize> {
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    q.questions[0].selected
}

fn question_selected(s: &AppState, idx: usize) -> Option<usize> {
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    q.questions[idx].selected
}

fn question_state_multi(items: Vec<crate::state::QuestionItem>) -> AppState {
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Question(
        crate::state::QuestionPromptState {
            request_id: "q1".into(),
            original_input: serde_json::json!({}),
            questions: items,
            current_question: crate::state::QuestionPage::Question(0),
            focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
            is_in_plan_mode: false,
        },
    ));
    s
}

fn focused_page(s: &AppState) -> crate::state::QuestionPage {
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    q.current_question
}

fn focused_target(s: &AppState) -> crate::state::QuestionFocusTarget {
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    q.focus_target
}

fn set_page(s: &mut AppState, page: crate::state::QuestionPage) {
    if let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_mut() {
        match page {
            crate::state::QuestionPage::Question(idx) => q.set_question_page(idx),
            crate::state::QuestionPage::Submit => q.set_submit_page(),
        }
    }
}

fn set_target(s: &mut AppState, target: crate::state::QuestionFocusTarget) {
    if let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_mut() {
        q.focus_target = target;
        q.sync_other_focus();
    }
}

#[test]
fn question_switch_question_walks_questions_then_submit_with_wrap() {
    use crate::state::QuestionPage;
    // 3 questions → nav ring [Q0, Q1, Q2, Submit].
    let mut s = question_state_multi(vec![
        question_item(vec![question_option("A"), question_option("B")], false),
        question_item(vec![question_option("C"), question_option("D")], false),
        question_item(vec![question_option("E"), question_option("F")], false),
    ]);
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(focused_page(&s), QuestionPage::Question(1));
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(focused_page(&s), QuestionPage::Question(2));
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(
        focused_page(&s),
        QuestionPage::Submit,
        "after the last question → Submit tab"
    );
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(
        focused_page(&s),
        QuestionPage::Question(0),
        "Submit wraps to the first question"
    );
    crate::bottom_pane::question::question_switch_question(&mut s, -1);
    assert_eq!(
        focused_page(&s),
        QuestionPage::Submit,
        "Left from the first question wraps to Submit"
    );
}

#[test]
fn question_switch_question_from_footer_keeps_footer_out_of_page_state() {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionFooterAction;
    use crate::state::QuestionPage;
    // 2 questions → ring [Q0, Q1, Submit].
    let mut s = question_state_multi(vec![
        question_item(vec![question_option("A"), question_option("B")], false),
        question_item(vec![question_option("C"), question_option("D")], false),
    ]);
    set_target(
        &mut s,
        QuestionFocusTarget::QuestionFooter(QuestionFooterAction::ChatAboutThis),
    );
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(
        focused_page(&s),
        QuestionPage::Question(1),
        "Right from footer advances page without falling back to Q0"
    );
    set_target(
        &mut s,
        QuestionFocusTarget::QuestionFooter(QuestionFooterAction::ChatAboutThis),
    );
    crate::bottom_pane::question::question_switch_question(&mut s, -1);
    assert_eq!(
        focused_page(&s),
        QuestionPage::Question(0),
        "Left from footer returns to the previous question page"
    );
}

#[test]
fn nav_on_submit_tab_toggles_submit_and_cancel() {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionPage;
    use crate::state::SubmitAction;
    let mut s = question_state_multi(vec![
        question_item(vec![question_option("A"), question_option("B")], false),
        question_item(vec![question_option("C"), question_option("D")], false),
    ]);
    set_page(&mut s, QuestionPage::Submit);
    assert_eq!(
        focused_target(&s),
        QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers),
        "starts on Submit"
    );
    super::nav(&mut s, 1);
    assert_eq!(
        focused_target(&s),
        QuestionFocusTarget::SubmitAction(SubmitAction::Cancel),
        "Down → Cancel"
    );
    super::nav(&mut s, 1);
    assert_eq!(
        focused_target(&s),
        QuestionFocusTarget::SubmitAction(SubmitAction::Cancel),
        "clamps at Cancel"
    );
    super::nav(&mut s, -1);
    assert_eq!(
        focused_target(&s),
        QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers),
        "Up → Submit"
    );
}

#[test]
fn question_switch_question_single_question_is_noop() {
    use crate::state::QuestionPage;
    let mut s = question_state(question_item(
        vec![question_option("A"), question_option("B")],
        false,
    ));
    crate::bottom_pane::question::question_switch_question(&mut s, 1);
    assert_eq!(focused_page(&s), QuestionPage::Question(0));
}

#[test]
fn question_digit_shortcut_moves_focus_to_that_option() {
    let mut s = question_state(question_item(
        vec![
            question_option("A"),
            question_option("B"),
            question_option("C"),
        ],
        false,
    ));
    crate::bottom_pane::question::question_select_digit(&mut s, 3);
    assert_eq!(
        focused_target(&s),
        crate::state::QuestionFocusTarget::QuestionOption(2)
    );
    assert_eq!(focused_selected(&s), None, "focus alone must not commit");
    // Out-of-range digit is a no-op.
    crate::bottom_pane::question::question_select_digit(&mut s, 9);
    assert_eq!(
        focused_target(&s),
        crate::state::QuestionFocusTarget::QuestionOption(2)
    );
}

#[test]
fn question_digit_routes_through_filter_when_not_editing_other_as_focus_only() {
    let mut s = question_state(question_item(
        vec![question_option("A"), question_option("B")],
        false,
    ));
    super::filter(&mut s, '2');
    assert_eq!(
        focused_target(&s),
        crate::state::QuestionFocusTarget::QuestionOption(1)
    );
    assert_eq!(focused_selected(&s), None, "filter has no command channel");
}

#[test]
fn nav_moves_focus_without_changing_committed_single_select_answer() {
    let mut s = question_state(question_item(
        vec![
            question_option("A"),
            question_option("B"),
            question_option("C"),
        ],
        false,
    ));
    super::nav(&mut s, 1);
    assert_eq!(
        focused_target(&s),
        crate::state::QuestionFocusTarget::QuestionOption(1)
    );
    assert_eq!(focused_selected(&s), None);
}

#[tokio::test]
async fn enter_commits_focused_option_before_submitting() {
    let mut s = question_state(question_item(
        vec![question_option("A"), question_option("B")],
        false,
    ));
    set_target(&mut s, crate::state::QuestionFocusTarget::QuestionOption(1));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    super::confirm(&mut s, &tx).await;

    let Ok(UserCommand::ApprovalResponse {
        approved,
        updated_input: Some(payload),
        ..
    }) = rx.try_recv()
    else {
        panic!("expected approval response");
    };
    assert!(approved);
    assert_eq!(payload["answers"]["Q?"], "B");
}

#[tokio::test]
async fn digit_shortcut_commits_and_advances_to_next_question() {
    let mut s = question_state_multi(vec![
        question_item(vec![question_option("A"), question_option("B")], false),
        question_item(vec![question_option("C"), question_option("D")], false),
    ]);
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    assert!(crate::bottom_pane::question::question_select_digit_and_confirm(&mut s, 2, &tx).await);

    assert!(rx.try_recv().is_err(), "first question should not submit");
    assert_eq!(focused_page(&s), crate::state::QuestionPage::Question(1));
    assert_eq!(question_selected(&s, 0), Some(1));
}

#[tokio::test]
async fn digit_shortcut_focuses_free_text_without_committing_it() {
    let mut s = question_state(question_item(
        vec![question_option("A"), question_option("B")],
        false,
    ));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    assert!(crate::bottom_pane::question::question_select_digit_and_confirm(&mut s, 3, &tx).await);

    assert!(rx.try_recv().is_err(), "free-text shortcut only focuses");
    assert_eq!(
        focused_target(&s),
        crate::state::QuestionFocusTarget::OtherInput
    );
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    assert!(!q.questions[0].other_input.committed);
}

#[test]
fn multi_select_empty_submits_empty_answer_like_ts() {
    // An untouched multi-select question submits an empty answer (the
    // selected array verbatim), NOT the cursor option.
    let q = crate::state::QuestionPromptState {
        request_id: "q1".into(),
        original_input: serde_json::json!({}),
        questions: vec![question_item(
            vec![question_option("A"), question_option("B")],
            true,
        )],
        current_question: crate::state::QuestionPage::Question(0),
        focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
        is_in_plan_mode: false,
    };
    let payload = crate::bottom_pane::question::build_answer_payload(&q);
    let answer = payload["answers"]["Q?"].as_str().unwrap();
    assert_eq!(answer, "", "untouched multi-select must submit empty");
}

#[test]
fn question_free_text_paste_appends_into_focused_free_text_input() {
    let item = question_item(vec![question_option("A")], false);
    let mut s = question_state(item);
    set_target(&mut s, crate::state::QuestionFocusTarget::OtherInput);
    assert!(crate::bottom_pane::question::question_free_text_paste(
        &mut s,
        "够清楚"
    ));
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    assert_eq!(
        q.questions[0].other_input.value, "够清楚",
        "paste lands in Other input"
    );
}

#[test]
fn question_free_text_paste_ignored_when_free_text_not_focused() {
    let item = question_item(vec![question_option("A")], false);
    let mut s = question_state(item);
    assert!(!crate::bottom_pane::question::question_free_text_paste(
        &mut s, "x"
    ));
}

#[tokio::test]
async fn confirm_on_empty_free_text_keeps_prompt_open_instead_of_submitting() {
    let item = question_item(vec![question_option("A")], false);
    let mut s = question_state(item);
    set_target(&mut s, crate::state::QuestionFocusTarget::OtherInput);
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    super::confirm(&mut s, &tx).await;
    assert!(rx.try_recv().is_err(), "empty Other must not submit");
    assert!(
        s.ui.interaction.active_prompt.is_some(),
        "prompt stays open so the user can type directly"
    );
}

#[tokio::test]
async fn confirm_on_free_text_with_value_submits() {
    let mut item = question_item(vec![question_option("A")], false);
    item.other_input.value = "my answer".into();
    let mut s = question_state(item);
    set_target(&mut s, crate::state::QuestionFocusTarget::OtherInput);
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    super::confirm(&mut s, &tx).await;
    let Ok(UserCommand::ApprovalResponse {
        approved,
        updated_input: Some(payload),
        ..
    }) = rx.try_recv()
    else {
        panic!("expected ApprovalResponse");
    };
    assert!(approved, "non-empty Other on the last question submits");
    assert_eq!(payload["answers"]["Q?"], "my answer");
    assert!(
        !payload["answers"]["Q?"]
            .as_str()
            .unwrap_or_default()
            .contains('A'),
        "free-text answer should not include the prior option"
    );
}

#[tokio::test]
async fn enter_on_sandbox_prompt_restores_it_without_dropping_the_request() {
    // RC-3: Enter is not a decision key for a binary sandbox approval. The old
    // code dismissed the prompt on Enter, orphaning the engine request (it hung
    // until interrupt). Enter must keep the prompt answerable and must NOT
    // auto-approve the escalation.
    use crate::state::SandboxPermissionPromptState;
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::SandboxPermission(
        SandboxPermissionPromptState {
            request_id: "sandbox-1".into(),
            description: "Sandbox access requested".into(),
        },
    ));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    super::confirm(&mut s, &tx).await;
    assert!(
        matches!(
            s.ui.interaction.active_prompt,
            Some(PanePromptState::SandboxPermission(_))
        ),
        "Enter keeps the sandbox prompt open (answerable), not dropped",
    );
    assert!(
        rx.try_recv().is_err(),
        "Enter must not auto-approve sandbox"
    );
}

#[tokio::test]
async fn enter_on_plan_entry_prompt_confirms_it() {
    // Plan entry is a benign confirmation (not a privilege escalation): Enter
    // commits it exactly like `y`, toggling plan mode and dismissing the
    // prompt. It must not be a silent no-op like the escalation prompts.
    use crate::state::PlanEntryPromptState;
    let mut s = AppState::new();
    assert_eq!(
        s.session.permission_mode,
        coco_types::PermissionMode::Default
    );
    s.ui.push_prompt(PanePromptState::PlanEntry(PlanEntryPromptState {
        description: "Enter plan mode?".into(),
    }));
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    super::confirm(&mut s, &tx).await;
    assert_eq!(
        s.session.permission_mode,
        coco_types::PermissionMode::Plan,
        "Enter confirms plan entry, toggling into Plan mode",
    );
    assert!(
        s.ui.interaction.active_prompt.is_none(),
        "the plan-entry prompt is dismissed after Enter",
    );
    assert!(
        matches!(rx.try_recv(), Ok(UserCommand::SetPermissionMode { .. })),
        "confirming plan entry forwards the mode change to core",
    );
}

#[tokio::test]
async fn approve_falls_through_to_modal_when_a_modal_is_open() {
    // RC-3 (M4): a modal renders on top of any bottom-pane prompt and owns the
    // keys. With a prompt hidden beneath an open modal, Approve must NOT resolve
    // the hidden prompt — it falls through so the modal handles the key.
    use crate::state::PermissionDetail;
    use crate::state::PermissionPromptState;
    let mut s = AppState::new();
    s.ui.push_prompt(PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        description: "Run".into(),
        detail: PermissionDetail::Generic {
            input_preview: "ls".into(),
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("ls".into()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }));
    s.ui.show_modal(ModalState::Help);
    let (tx, _rx) = mpsc::channel::<UserCommand>(8);
    let routed = crate::bottom_pane::route_approve(&mut s, &tx).await;
    assert!(
        !routed,
        "an open modal must take precedence over the hidden prompt",
    );
    assert!(
        s.ui.interaction.active_prompt.is_some(),
        "the hidden prompt is left untouched for the modal to coexist with",
    );
}

#[test]
fn space_types_into_focused_other_input() {
    // RC-3 (M5): Space must be typeable into the focused "Other" free-text field
    // (multi-word answers). The old order checked Space → toggle BEFORE the
    // free-text path, so a space never reached the focused input.
    let item = question_item(vec![question_option("A")], false);
    let mut s = question_state(item);
    set_target(&mut s, crate::state::QuestionFocusTarget::OtherInput);
    super::filter(&mut s, ' ');
    let Some(PanePromptState::Question(q)) = s.ui.interaction.active_prompt.as_ref() else {
        panic!("expected a question prompt");
    };
    assert_eq!(
        q.questions[0].other_input.value, " ",
        "space lands in the focused Other input",
    );
}
