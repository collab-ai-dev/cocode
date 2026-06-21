use super::*;
use coco_types::PermissionAskChoice;
use pretty_assertions::assert_eq;
use ratatui::style::Modifier;
use ratatui::text::Line;
use serde_json::json;

use crate::i18n::locale_test_guard;
use crate::state::OtherInputState;
use crate::state::PermissionDetail;
use crate::state::QuestionFocusTarget;
use crate::state::QuestionItem;
use crate::state::QuestionOption;
use crate::state::QuestionPage;
use crate::state::SubmitAction;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::QuestionRow;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn permission_prompt(detail: PermissionDetail) -> PermissionPromptState {
    let display_input = match &detail {
        PermissionDetail::Generic { input_preview } => {
            coco_types::PermissionDisplayInput::Text(input_preview.clone())
        }
        _ => coco_types::PermissionDisplayInput::Empty,
    };
    PermissionPromptState {
        request_id: "req-1".to_string(),
        tool_name: "Edit".to_string(),
        description: "Allow this operation?".to_string(),
        detail,
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input,
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

#[test]
fn permission_styled_content_highlights_selected_action() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "Read file".to_string(),
    });
    state.tool_name = "Read".to_string();
    state.show_always_allow = true;
    state.selected_choice = 2;
    state.original_input = Some(json!({"file_path": "/tmp/proj/notes.md"}));

    let (_, lines, _) = permission_styled_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    let selected = lines
        .iter()
        .find(|line| line_text(line).contains("always allow Read locally"))
        .expect("selected row");
    let style = selected.spans.first().expect("row span").style;

    assert_eq!(style.fg, Some(theme.selection_fg));
    assert_eq!(style.bg, Some(theme.selection_bg));
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(line_text(selected).starts_with("▸ "));
    assert!(line_text(selected).ends_with(" ◂"));
}

#[test]
fn shell_prompt_renders_editable_prefix_with_cursor() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Bash {
        command: "git status -s".to_string(),
        risk_description: None,
        working_dir: None,
    });
    state.tool_name = "Bash".to_string();
    state.show_always_allow = true;
    state.original_input = Some(json!({"command": "git status -s"}));
    state.prefix_input = Some(crate::state::PrefixInputState::new(
        "git status:*".to_string(),
    ));
    // Focus the local allow row (actions: Yes, session, local, No → idx 2).
    state.selected_choice = 2;

    let (_, lines, _) = permission_styled_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    // The focused allow row shows the editable rule prefix with a cursor glyph.
    let focused = lines
        .iter()
        .map(line_text)
        .find(|t| t.contains('▏'))
        .expect("focused row with cursor");
    assert!(focused.contains("git status:*"), "got: {focused}");
    // The unfocused session row shows the same prefix without a cursor.
    let session = lines
        .iter()
        .map(line_text)
        .find(|t| t.contains("git status:*") && !t.contains('▏'))
        .expect("unfocused session prefix row");
    assert!(session.contains("git status:*"));
    // The hint switches to the edit affordance while an allow row is focused.
    let hints = lines.iter().any(|l| line_text(l).contains("type to edit"));
    assert!(hints, "edit hint missing");
}

#[test]
fn permission_content_hints_approve_deny_only_shortcuts() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = permission_prompt(PermissionDetail::Generic {
        input_preview: "Run tool".to_string(),
    });

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(body.contains("1-2 or Y/N shortcuts"), "body: {body}");
}

#[test]
fn permission_content_hints_add_directories_session_and_local() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "Add directory".to_string(),
    });
    state.show_always_allow = true;
    state.permission_suggestions = vec![coco_types::PermissionUpdate::AddDirectories {
        directories: vec!["/tmp/project".into()],
        destination: coco_types::PermissionUpdateDestination::Session,
    }];

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    // The dir suggestion now drives both session and local rows → 4 rows.
    assert!(body.contains("2/s · Yes, allow Edit for this session"));
    assert!(body.contains("3/a · Yes, always allow Edit locally"));
    assert!(body.contains("1-4 or Y/S/A/N shortcuts"), "body: {body}");
}

#[test]
fn permission_content_hints_session_and_local_shortcuts() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "Read file".to_string(),
    });
    state.tool_name = "Read".to_string();
    state.show_always_allow = true;
    state.original_input = Some(json!({"file_path": "/tmp/project/notes.md"}));

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(body.contains("2/s · Yes, allow Read for this session"));
    assert!(body.contains("3/a · Yes, always allow Read locally"));
    assert!(body.contains("1-4 or Y/S/A/N shortcuts"), "body: {body}");
}

#[test]
fn permission_content_title_includes_worker_badge() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "ls".to_string(),
    });
    state.worker_badge = Some(coco_types::WorkerBadge {
        name: "researcher".to_string(),
        color: coco_types::AgentColorName::Cyan,
    });
    let (title, _body, _border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    // The worker name is surfaced in the title so the leader sees who is asking.
    assert!(title.contains("· @researcher"), "got title: {title}");
}

#[test]
fn permission_content_omits_badge_without_worker() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = permission_prompt(PermissionDetail::Generic {
        input_preview: "ls".to_string(),
    });
    let (title, _body, _border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    assert!(!title.contains('@'), "no badge expected: {title}");
}

#[test]
fn permission_content_uses_high_risk_border() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Bash {
        command: "rm -rf target/tmp".to_string(),
        risk_description: Some("Deletes files".to_string()),
        working_dir: Some("/repo".to_string()),
    });
    state.risk_level = Some(RiskLevel::High);
    state.show_always_allow = true;
    state.original_input = Some(json!({"file_path": "/tmp/proj/src/main.rs"}));

    let (title, body, border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert_eq!(border, theme.error);
    assert!(title.contains("Edit"));
    assert!(body.contains("rm -rf target/tmp"));
    assert!(body.contains("/repo"));
    assert!(body.contains("Deletes files"));
    assert!(body.contains("always allow Edit locally"));
}

#[test]
fn permission_content_renders_choices_instead_of_default_actions() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "Pick an option".to_string(),
    });
    state.show_always_allow = true;
    state.selected_choice = 1;
    state.choices = Some(vec![
        PermissionAskChoice {
            value: "keep".to_string(),
            label: "Keep context".to_string(),
            description: Some("Continue with current context".to_string()),
        },
        PermissionAskChoice {
            value: "clear".to_string(),
            label: "Clear context".to_string(),
            description: Some("Start a smaller plan".to_string()),
        },
    ]);

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(body.contains("  Keep context"));
    assert!(body.contains("▸ Clear context"));
    assert!(body.contains("Start a smaller plan"));
    assert!(!body.contains("Always"));
}

#[test]
fn permission_content_renders_exit_plan_mode_without_raw_input_keys() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
        plan: Some("# Plan\n\n- Update code".to_string()),
        plan_file_path: Some("/tmp/plan.md".to_string()),
        allowed_prompts: vec![],
    });
    state.tool_name = coco_types::ToolName::ExitPlanMode.as_str().to_string();
    state.description = "Exit plan mode?".to_string();
    state.choices = Some(vec![
        PermissionAskChoice {
            value: "yes-accept-edits-keep-context".to_string(),
            label: "Yes, auto-accept edits".to_string(),
            description: None,
        },
        PermissionAskChoice {
            value: "yes-default-keep-context".to_string(),
            label: "Yes, manually approve edits".to_string(),
            description: None,
        },
        PermissionAskChoice {
            value: "no".to_string(),
            label: "No, keep planning".to_string(),
            description: None,
        },
    ]);

    let (title, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(title.contains("Ready to code?"));
    assert!(
        !body.contains("Here is proposed plan:"),
        "generic permission body must not render ExitPlanMode plan text: {body}"
    );
    assert!(!body.contains("- Update code"), "{body}");
    assert!(!body.contains("Plan file: /tmp/plan.md"), "{body}");
    assert!(!body.contains("Requested permissions:"));
    assert!(!body.contains("Bash(prompt: cargo test)"));
    assert!(body.contains("Yes, manually approve edits"));
    assert!(body.contains("No, keep planning"));
    assert!(!body.contains("planFilePath"));
    assert!(!body.contains("allowedPrompts"));
    assert!(!body.contains("Exit plan mode?"));
}

#[test]
fn permission_content_uses_no_plan_title_for_exit_plan_without_plan() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
        plan: Some("User asked for a read-only explanation.".to_string()),
        plan_file_path: None,
        allowed_prompts: vec![],
    });

    let (title, _, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(title.contains("Exit plan mode?"), "{title}");
    assert!(!title.contains("Ready to code?"), "{title}");
}

#[test]
fn exit_plan_prompt_lines_render_only_decision_rows() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
        plan: Some("# Goal\n\n- Step one".to_string()),
        plan_file_path: Some("/tmp/plan.md".to_string()),
        allowed_prompts: vec![],
    });
    state.selected_choice = 1;
    state.choices = Some(vec![
        PermissionAskChoice {
            value: coco_types::ExitPlanChoice::KeepAcceptEdits
                .as_str()
                .to_string(),
            label: "Yes, auto-accept edits".to_string(),
            description: None,
        },
        PermissionAskChoice {
            value: coco_types::ExitPlanChoice::KeepDefault.as_str().to_string(),
            label: "Yes, manually approve edits".to_string(),
            description: Some("Ask before each edit.".to_string()),
        },
        PermissionAskChoice {
            value: coco_types::ExitPlanChoice::No.as_str().to_string(),
            label: "No, keep planning".to_string(),
            description: None,
        },
    ]);

    let lines = exit_plan_prompt_lines(&state, UiStyles::new(&theme), 8);
    let joined = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    // The "Ready to code?" title now leads the plan block in the transcript,
    // not the bottom prompt — the prompt keeps only the proceed cue + choices.
    assert!(!joined.contains("Ready to code?"), "{joined}");
    assert!(joined.contains("Would you like to proceed?"), "{joined}");
    assert!(joined.contains("Yes, manually approve edits"), "{joined}");
    assert!(joined.contains("Ask before each edit."), "{joined}");
    assert!(!joined.contains("Step one"), "{joined}");
    assert!(!joined.contains("Plan file:"), "{joined}");
}

#[test]
fn exit_plan_prompt_lines_use_no_plan_copy_without_plan() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
        plan: Some("This was a read-only question.".to_string()),
        plan_file_path: None,
        allowed_prompts: vec![],
    });
    state.choices = Some(vec![
        PermissionAskChoice {
            value: coco_types::ExitPlanChoice::KeepDefault.as_str().to_string(),
            label: "Yes, exit plan mode".to_string(),
            description: None,
        },
        PermissionAskChoice {
            value: coco_types::ExitPlanChoice::No.as_str().to_string(),
            label: "No, keep planning".to_string(),
            description: None,
        },
    ]);

    let lines = exit_plan_prompt_lines(&state, UiStyles::new(&theme), 8);
    let joined = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    // No-plan exit: the bottom prompt drops the bold title too, leaving the
    // descriptive proceed line + choices.
    assert!(joined.contains("Would you like to proceed?"), "{joined}");
    assert!(
        joined.contains("without an implementation plan"),
        "{joined}"
    );
    assert!(joined.contains("Yes, exit plan mode"), "{joined}");
    assert!(joined.contains("No, keep planning"), "{joined}");
    assert!(!joined.contains("auto-accept"), "{joined}");
    assert!(!joined.contains("manually approve edits"), "{joined}");
    assert!(!joined.contains("Ready to code?"), "{joined}");
    assert!(
        lines.iter().any(|line| line_text(line).is_empty()),
        "prompt should leave space before choices: {joined}"
    );
}

#[test]
fn exit_plan_pending_history_lines_render_plan_without_choices() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
        plan: Some("# Goal\n\n- Step one".to_string()),
        plan_file_path: Some("/tmp/plan.md".to_string()),
        allowed_prompts: vec![],
    });
    state.choices = Some(vec![PermissionAskChoice {
        value: coco_types::ExitPlanChoice::KeepDefault.as_str().to_string(),
        label: "Yes, manually approve edits".to_string(),
        description: None,
    }]);

    let lines = exit_plan_pending_history_lines(
        &state,
        /*width*/ 80,
        coco_tui_ui::display::SyntaxHighlighting::Disabled,
        UiStyles::new(&theme),
    );
    let joined = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(joined.contains("Here is proposed plan:"), "{joined}");
    assert!(joined.contains("Goal"), "{joined}");
    assert!(joined.contains("Step one"), "{joined}");
    assert!(joined.contains("Plan file: /tmp/plan.md"), "{joined}");
    assert!(!joined.contains("manually approve"), "{joined}");
}

#[test]
fn exit_plan_pending_history_lines_lead_with_ready_to_code_title() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
        plan: Some("# Goal\n\n- Step one".to_string()),
        plan_file_path: None,
        allowed_prompts: vec![],
    });

    let lines = exit_plan_pending_history_lines(
        &state,
        /*width*/ 80,
        coco_tui_ui::display::SyntaxHighlighting::Disabled,
        UiStyles::new(&theme),
    );
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    let joined = texts.join("\n");

    // TS-style "title in front": the bold "Ready to code?" header leads, the
    // plan heading and body follow.
    let ready_at = texts
        .iter()
        .position(|t| t.contains("Ready to code?"))
        .unwrap_or_else(|| panic!("missing title: {joined}"));
    let heading_at = texts
        .iter()
        .position(|t| t.contains("Here is proposed plan:"))
        .unwrap_or_else(|| panic!("missing heading: {joined}"));
    assert!(ready_at < heading_at, "title must precede plan: {joined}");
    assert!(joined.contains("Step one"), "{joined}");
}

#[test]
fn exit_plan_pending_history_lines_render_no_plan_notice_without_file() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = permission_prompt(PermissionDetail::ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
        plan: Some("User asked for a read-only explanation.".to_string()),
        plan_file_path: Some("/tmp/plan.md".to_string()),
        allowed_prompts: vec![],
    });

    let lines = exit_plan_pending_history_lines(
        &state,
        /*width*/ 80,
        coco_tui_ui::display::SyntaxHighlighting::Disabled,
        UiStyles::new(&theme),
    );
    let joined = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(
        joined.contains("No implementation plan needed."),
        "{joined}"
    );
    assert!(!joined.contains("Here is proposed plan:"), "{joined}");
    assert!(!joined.contains("Plan file:"), "{joined}");
    assert!(!joined.contains("read-only explanation"), "{joined}");
}

#[test]
fn generic_permission_content_uses_display_input_not_raw_original_input() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "safe display".to_string(),
    });
    state.original_input = Some(json!({"secret": "raw value"}));
    state.display_input = coco_types::PermissionDisplayInput::Json("{\"safe\":true}".to_string());

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(body.contains("{\"safe\":true}"));
    assert!(!body.contains("raw value"));
}

#[test]
fn permission_content_truncates_unicode_file_edit_preview() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let diff = "切".repeat(501);
    let state = permission_prompt(PermissionDetail::FileEdit {
        path: "src/lib.rs".to_string(),
        diff,
    });

    let (_, body, _) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );

    assert!(body.contains("File: src/lib.rs"));
    assert!(body.contains(&format!("{}...", "切".repeat(500))));
}

fn question_prompt(question: QuestionItem) -> QuestionPromptState {
    QuestionPromptState {
        request_id: "req-1".to_string(),
        original_input: json!({"source": "test"}),
        questions: vec![question],
        current_question: QuestionPage::Question(0),
        focus_target: QuestionFocusTarget::QuestionOption(0),
        is_in_plan_mode: false,
    }
}

fn choice_mark(row: &QuestionRow) -> &RowMark {
    let QuestionRow::Choice(choice) = row else {
        panic!("expected choice row");
    };
    &choice.mark
}

fn action_label(row: &QuestionRow) -> &str {
    let QuestionRow::Action(action) = row else {
        panic!("expected action row");
    };
    &action.label
}

#[test]
fn project_question_exposes_other_composer_buffer() {
    let _locale = locale_test_guard("en");
    let state = question_prompt(QuestionItem {
        header: "Auth".to_string(),
        question: "Which auth flow?".to_string(),
        options: vec![QuestionOption {
            label: "OAuth".to_string(),
            description: "Use browser login".to_string(),
            preview: None,
        }],
        multi_select: false,
        selected: None,
        checked: Vec::new(),
        other_input: OtherInputState {
            focused: true,
            value: "device code".to_string(),
            committed: false,
        },
    });
    let mut state = state;
    state.focus_target = QuestionFocusTarget::OtherInput;

    let view = project_question(&state);

    assert_eq!(view.header.title, " Question ");
    assert_eq!(view.header.chip.as_deref(), Some("Auth"));
    assert_eq!(view.rows.len(), 2);
    let QuestionRow::Input(input) = &view.rows[1] else {
        panic!("expected trailing input row");
    };
    assert_eq!(input.number, 2);
    assert_eq!(input.value, "device code");
    assert!(!input.selected);
    assert!(input.focused);
}

#[test]
fn project_question_initial_single_select_has_focus_but_no_committed_check() {
    let _locale = locale_test_guard("en");
    let state = question_prompt(QuestionItem {
        header: "Auth".to_string(),
        question: "Which auth flow?".to_string(),
        options: vec![
            QuestionOption {
                label: "OAuth".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "Device".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        selected: None,
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    });

    let view = project_question(&state);

    assert!(matches!(
        choice_mark(&view.rows[0]),
        RowMark::Radio {
            selected: false,
            focused: true
        }
    ));
}

#[test]
fn project_question_committed_single_select_shows_check_separate_from_focus() {
    let _locale = locale_test_guard("en");
    let mut state = question_prompt(QuestionItem {
        header: "Auth".to_string(),
        question: "Which auth flow?".to_string(),
        options: vec![
            QuestionOption {
                label: "OAuth".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "Device".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        selected: Some(0),
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    });
    state.focus_target = QuestionFocusTarget::QuestionOption(1);

    let view = project_question(&state);

    assert!(matches!(
        choice_mark(&view.rows[0]),
        RowMark::Radio {
            selected: true,
            focused: false
        }
    ));
    assert!(matches!(
        choice_mark(&view.rows[1]),
        RowMark::Radio {
            selected: false,
            focused: true
        }
    ));
}

#[test]
fn project_question_uncommitted_free_text_value_has_no_check() {
    let _locale = locale_test_guard("en");
    let mut state = question_prompt(QuestionItem {
        header: "Auth".to_string(),
        question: "Which auth flow?".to_string(),
        options: vec![QuestionOption {
            label: "OAuth".to_string(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        selected: Some(0),
        checked: Vec::new(),
        other_input: OtherInputState {
            focused: true,
            value: "device code".to_string(),
            committed: false,
        },
    });
    state.focus_target = QuestionFocusTarget::OtherInput;

    let view = project_question(&state);

    let QuestionRow::Input(input) = &view.rows[1] else {
        panic!("expected free-text row");
    };
    assert!(!input.selected, "typing alone must not show a check");
}

#[test]
fn project_question_committed_free_text_value_shows_check() {
    let _locale = locale_test_guard("en");
    let mut state = question_prompt(QuestionItem {
        header: "Auth".to_string(),
        question: "Which auth flow?".to_string(),
        options: vec![QuestionOption {
            label: "OAuth".to_string(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        selected: None,
        checked: Vec::new(),
        other_input: OtherInputState {
            focused: true,
            value: "device code".to_string(),
            committed: true,
        },
    });
    state.focus_target = QuestionFocusTarget::OtherInput;

    let view = project_question(&state);

    let QuestionRow::Input(input) = &view.rows[1] else {
        panic!("expected free-text row");
    };
    assert!(
        input.selected,
        "Enter-confirmed free text should show a check"
    );
}

#[test]
fn project_question_truncates_long_header_chip_to_12() {
    let _locale = locale_test_guard("en");
    let state = question_prompt(QuestionItem {
        header: "Authentication method".to_string(),
        question: "Which?".to_string(),
        options: vec![
            QuestionOption {
                label: "A".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "B".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        selected: None,
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    });

    let view = project_question(&state);
    // CHIP_MAX_CHARS = 12: 11 chars + ellipsis.
    assert_eq!(view.header.chip.as_deref(), Some("Authenticat…"));
    assert_eq!(view.header.chip.as_deref().unwrap().chars().count(), 12);
}

#[test]
fn project_question_multiselect_footer_and_hints() {
    let _locale = locale_test_guard("en");
    let mut state = question_prompt(QuestionItem {
        header: "Tools".to_string(),
        question: "Pick tools".to_string(),
        options: vec![
            QuestionOption {
                label: "Read".to_string(),
                description: String::new(),
                preview: Some("read preview".to_string()),
            },
            QuestionOption {
                label: "Write".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: true,
        selected: None,
        checked: vec![0],
        other_input: OtherInputState::default(),
    });
    state.is_in_plan_mode = true;

    let view = project_question(&state);

    assert!(matches!(
        choice_mark(&view.rows[0]),
        RowMark::Check {
            checked: true,
            focused: true
        }
    ));
    assert_eq!(view.preview.as_deref(), Some("read preview"));
    // Plan mode adds the Skip-interview footer action.
    assert_eq!(view.footer_actions.len(), 2);
    assert!(
        view.footer_actions
            .iter()
            .any(|f| f.label.contains("Skip interview"))
    );
    assert!(view.hints.contains("Space to toggle"));
}

#[test]
fn project_question_nav_answered_reflects_each_question() {
    let _locale = locale_test_guard("en");
    let q = |header: &str, multi: bool| QuestionItem {
        header: header.to_string(),
        question: format!("{header}?"),
        options: vec![
            QuestionOption {
                label: "A".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "B".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: multi,
        selected: None,
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    };
    let state = QuestionPromptState {
        request_id: "r".to_string(),
        original_input: json!({}),
        // Q1 single-select starts uncommitted → unanswered; Q2
        // multi-select with nothing checked → unanswered.
        questions: vec![q("One", false), q("Two", true)],
        current_question: QuestionPage::Question(0),
        focus_target: QuestionFocusTarget::QuestionOption(0),
        is_in_plan_mode: false,
    };
    let nav = project_question(&state)
        .header
        .nav
        .expect("multi-question nav strip");
    assert!(!nav.tabs[0].answered, "single-select starts unanswered");
    assert!(!nav.tabs[1].answered, "empty multi-select is unanswered");
    // The Submit tab is present; not yet ready (Q2 unanswered).
    let submit = nav.submit.expect("submit tab present with >1 question");
    assert!(!submit.ready, "ready only when every question is answered");
}

#[test]
fn project_question_submit_focus_shows_review_body_and_ready_tab() {
    let _locale = locale_test_guard("en");
    let q = |header: &str| QuestionItem {
        header: header.to_string(),
        question: format!("{header}?"),
        options: vec![
            QuestionOption {
                label: "A".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "B".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        selected: Some(0),
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    };
    let state = QuestionPromptState {
        request_id: "r".to_string(),
        original_input: json!({}),
        // Both single-select → pre-answered with their first option ("A").
        questions: vec![q("One"), q("Two")],
        current_question: QuestionPage::Submit,
        focus_target: QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers),
        is_in_plan_mode: false,
    };
    let view = project_question(&state);
    let submit = view
        .header
        .nav
        .expect("nav strip")
        .submit
        .expect("submit tab");
    assert!(submit.focused, "Submit tab is focused");
    assert!(submit.ready, "all questions answered → ✔");
    // Body = review of every answer + the "Ready to submit?" prompt.
    assert!(
        view.submit_review
            .as_ref()
            .is_some_and(|review| review.contains("Review your answers")),
        "review header: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    assert!(
        view.submit_review
            .as_ref()
            .is_some_and(|review| review.contains("One?") && review.contains("Two?")),
        "lists every question: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    assert!(
        view.submit_review
            .as_ref()
            .is_some_and(|review| review.contains("→ A")),
        "shows answers: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    assert!(
        view.submit_review
            .as_ref()
            .is_some_and(|review| review.contains("Ready to submit")),
        "submit prompt: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    assert!(
        !view
            .submit_review
            .as_ref()
            .is_some_and(|review| review.contains("not answered all")),
        "no warning when all answered: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    // Rows are the Submit / Cancel confirmation list.
    assert_eq!(view.rows.len(), 2, "Submit/Cancel rows");
    assert_eq!(action_label(&view.rows[0]), "Submit answers");
    assert_eq!(action_label(&view.rows[1]), "Cancel");
}

#[test]
fn project_question_submit_focus_warns_when_unanswered() {
    let _locale = locale_test_guard("en");
    let q = |header: &str, multi: bool| QuestionItem {
        header: header.to_string(),
        question: format!("{header}?"),
        options: vec![
            QuestionOption {
                label: "A".to_string(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "B".to_string(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: multi,
        selected: Some(0),
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    };
    let state = QuestionPromptState {
        request_id: "r".to_string(),
        original_input: json!({}),
        // Q2 multi-select with nothing checked → unanswered.
        questions: vec![q("One", false), q("Two", true)],
        current_question: QuestionPage::Submit,
        focus_target: QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers),
        is_in_plan_mode: false,
    };
    let view = project_question(&state);
    assert!(
        view.submit_review
            .as_ref()
            .is_some_and(|review| review.contains("⚠ You have not answered all questions")),
        "warning shown: {}",
        view.submit_review.as_deref().unwrap_or("")
    );
    assert!(
        !view.header.nav.unwrap().submit.unwrap().ready,
        "Submit tab not ready when a question is unanswered"
    );
}

#[test]
fn project_question_clamps_negative_focus_and_selection() {
    let _locale = locale_test_guard("en");
    let mut state = QuestionPromptState {
        request_id: "req-1".to_string(),
        original_input: json!({"source": "test"}),
        questions: vec![
            QuestionItem {
                header: "First".to_string(),
                question: "Pick first".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Alpha".to_string(),
                        description: String::new(),
                        preview: Some("alpha preview".to_string()),
                    },
                    QuestionOption {
                        label: "Beta".to_string(),
                        description: String::new(),
                        preview: None,
                    },
                ],
                multi_select: false,
                selected: Some(0),
                checked: Vec::new(),
                other_input: OtherInputState::default(),
            },
            QuestionItem {
                header: "Second".to_string(),
                question: "Pick second".to_string(),
                options: Vec::new(),
                multi_select: false,
                selected: None,
                checked: Vec::new(),
                other_input: OtherInputState::default(),
            },
        ],
        current_question: QuestionPage::Question(99),
        focus_target: QuestionFocusTarget::QuestionOption(0),
        is_in_plan_mode: false,
    };

    let view = project_question(&state);

    // >1 question → the nav strip (not a bare chip) carries every header,
    // current clamped to question 0.
    assert_eq!(view.header.chip, None);
    let nav = view.header.nav.as_ref().expect("multi-question nav strip");
    assert_eq!(nav.current, 1);
    assert_eq!(nav.tabs.len(), 2);
    assert_eq!(nav.tabs[0].header, "First");
    assert_eq!(nav.tabs[1].header, "Second");
    assert!(matches!(&view.rows[0], QuestionRow::Input(_)));
    assert_eq!(view.preview, None);

    state.current_question = QuestionPage::Question(0);
    state.focus_target = QuestionFocusTarget::QuestionOption(1);
    state.questions[0].selected = Some(1);
    let view = project_question(&state);
    // selected option 1 is focused.
    assert!(matches!(
        choice_mark(&view.rows[1]),
        RowMark::Radio {
            selected: true,
            focused: true
        }
    ));
}

#[test]
fn permission_content_explainer_panel_visible_high_risk() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "ls".to_string(),
    });
    state.explanation_visible = true;
    state.explanation = crate::state::ExplainerFetch::Ready(coco_types::PermissionExplanation {
        risk_level: coco_types::RiskLevel::High,
        explanation: "Deletes the production database.".to_string(),
        reasoning: "drop table".to_string(),
        risk: "data loss".to_string(),
    });

    let (_title, body, border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    assert!(body.contains("Risk: HIGH"), "body: {body}");
    assert!(
        body.contains("Deletes the production database."),
        "body: {body}"
    );
    // The border reflects the fetched risk while the panel is open.
    assert_eq!(border, theme.error);
}

#[test]
fn permission_content_explainer_hidden_when_collapsed() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "ls".to_string(),
    });
    // Fetched but the panel is collapsed → body must not show the explanation.
    state.explanation_visible = false;
    state.explanation = crate::state::ExplainerFetch::Ready(coco_types::PermissionExplanation {
        risk_level: coco_types::RiskLevel::High,
        explanation: "secret-detail".to_string(),
        reasoning: String::new(),
        risk: String::new(),
    });
    let (_title, body, _border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    assert!(!body.contains("Risk:"), "collapsed body leaked: {body}");
    assert!(
        !body.contains("secret-detail"),
        "collapsed body leaked: {body}"
    );
}

#[test]
fn permission_content_explainer_loading_line() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Generic {
        input_preview: "ls".to_string(),
    });
    state.explanation_visible = true;
    state.explanation = crate::state::ExplainerFetch::Loading;
    let (_title, body, _border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    assert!(body.contains("Analyzing risk"), "body: {body}");
}

#[test]
fn permission_content_explainer_panel_snapshot() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = permission_prompt(PermissionDetail::Bash {
        command: "rm -rf /tmp/cache".to_string(),
        risk_description: Some("Deletes cached files".to_string()),
        working_dir: Some("/repo".to_string()),
    });
    state.show_always_allow = true;
    state.explanation_visible = true;
    state.explanation = crate::state::ExplainerFetch::Ready(coco_types::PermissionExplanation {
        risk_level: coco_types::RiskLevel::High,
        explanation: "This removes the build cache directory.".to_string(),
        reasoning: "rm -rf is destructive".to_string(),
        risk: "data loss".to_string(),
    });
    let (_title, body, _border) = permission_content(
        &state,
        coco_types::PermissionMode::Default,
        UiStyles::new(&theme),
    );
    insta::assert_snapshot!(body);
}
