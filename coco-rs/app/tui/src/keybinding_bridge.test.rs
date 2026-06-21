//! Tests for keybinding bridge.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;

use crate::events::TuiCommand;
use crate::keybinding_bridge::KeybindingContext;
use crate::keybinding_bridge::active_context;
use crate::keybinding_bridge::map_key;
use crate::keybinding_bridge::should_log_key_command;
use crate::state::AppState;
use crate::state::PanePromptState;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn ctrl_shift(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn team_roster_state() -> AppState {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::TeamRoster(
        crate::state::TeamRosterState {
            team_name: "t".into(),
            members: Vec::new(),
            selected: 0,
        },
    ));
    state
}

fn model_picker_state() -> AppState {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::ModelPicker(
        crate::state::ModelPickerState {
            role: coco_types::ModelRole::Main,
            entries: vec![crate::state::ModelEntry {
                provider: "openai".into(),
                provider_display: "OpenAI".into(),
                model_id: "gpt-5-5".into(),
                display_name: "GPT-5.5".into(),
                context_window: Some(272_000),
                supported_efforts: vec![coco_types::ReasoningEffort::Auto],
                default_effort: Some(coco_types::ReasoningEffort::Auto),
                is_current_for_role: true,
                unavailable_reasons: Vec::new(),
            }],
            filter: String::new(),
            selected: 0,
            effort: Some(coco_types::ReasoningEffort::Auto),
        },
    ));
    state
}

fn copy_picker_state() -> AppState {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::CopyPicker(
        crate::state::CopyPickerState {
            full_text: "full".into(),
            code_blocks: vec![crate::state::CopyPickerCodeBlock {
                code: "code".into(),
                lang: Some("rust".into()),
            }],
            message_age: 0,
            selected: crate::state::CopyPickerSelection::Full,
        },
    ));
    state
}

fn install_permission_prompt(state: &mut AppState) {
    state.ui.push_prompt(PanePromptState::Permission(
        crate::state::PermissionPromptState {
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            description: "run".into(),
            detail: crate::state::PermissionDetail::Generic {
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
        },
    ));
}

#[test]
fn test_default_context_is_chat() {
    let state = AppState::new();
    assert_eq!(active_context(&state), KeybindingContext::Chat);
}

fn install_question_prompt(state: &mut AppState) {
    state.ui.push_prompt(PanePromptState::Question(
        crate::state::QuestionPromptState {
            request_id: "q1".into(),
            original_input: serde_json::json!({}),
            questions: vec![crate::state::QuestionItem {
                header: "Auth".into(),
                question: "Which?".into(),
                options: vec![crate::state::QuestionOption {
                    label: "OAuth".into(),
                    description: String::new(),
                    preview: None,
                }],
                multi_select: false,
                selected: None,
                checked: Vec::new(),
                other_input: crate::state::OtherInputState::default(),
            }],
            current_question: crate::state::QuestionPage::Question(0),
            focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
            is_in_plan_mode: false,
        },
    ));
}

#[test]
fn test_question_prompt_uses_dedicated_context() {
    let mut state = AppState::new();
    install_question_prompt(&mut state);
    assert_eq!(active_context(&state), KeybindingContext::Question);
}

#[test]
fn test_question_letter_keys_never_approve_or_deny() {
    // Regression for the C1 critical: routing AskUserQuestion through the
    // confirmation map made y/n/a emit Approve/Deny/ApproveAll, which tore the
    // prompt down with no answer (hung tool). They must route to the
    // filter/Other path instead; a question commits only via Enter.
    let mut state = AppState::new();
    install_question_prompt(&mut state);
    for c in ['y', 'n', 'a'] {
        assert!(
            matches!(
                map_key(&state, press(KeyCode::Char(c))),
                Some(TuiCommand::SurfaceFilter(got)) if got == c
            ),
            "'{c}' must route to the filter/Other path, not approve/deny",
        );
    }
    assert!(matches!(
        map_key(&state, press(KeyCode::Enter)),
        Some(TuiCommand::SurfaceConfirm)
    ));
    // C2 critical: the question free-text input needs a Backspace + printable route.
    assert!(matches!(
        map_key(&state, press(KeyCode::Backspace)),
        Some(TuiCommand::SurfaceFilterBackspace)
    ));
}

#[test]
fn test_help_modal_context() {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Help);
    assert_eq!(active_context(&state), KeybindingContext::Scrollable);
}

#[test]
fn test_transcript_modal_context() {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Transcript(
        crate::state::transcript::TranscriptState::new(),
    ));
    assert_eq!(active_context(&state), KeybindingContext::Transcript);
}

#[test]
fn test_transcript_modal_uses_pager_controls() {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Transcript(
        crate::state::transcript::TranscriptState::new(),
    ));

    assert!(matches!(
        map_key(&state, press(KeyCode::Up)),
        Some(TuiCommand::TranscriptScrollLines(-1))
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Down)),
        Some(TuiCommand::TranscriptScrollLines(1))
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::PageUp)),
        Some(TuiCommand::TranscriptPage(-1))
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::PageDown)),
        Some(TuiCommand::TranscriptPage(1))
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Home)),
        Some(TuiCommand::TranscriptJumpStart)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::End)),
        Some(TuiCommand::TranscriptJumpEnd)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Tab)),
        Some(TuiCommand::TranscriptSelectNext)
    ));
    assert!(map_key(&state, press(KeyCode::BackTab)).is_none());
    assert!(matches!(
        map_key(&state, press(KeyCode::Esc)),
        Some(TuiCommand::Cancel)
    ));
    assert!(matches!(
        map_key(&state, ctrl(KeyCode::Char('c'))),
        Some(TuiCommand::Interrupt)
    ));
    assert!(matches!(
        map_key(&state, ctrl(KeyCode::Char('d'))),
        Some(TuiCommand::RequestExit)
    ));
}

#[test]
fn test_permission_prompt_context() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    assert_eq!(active_context(&state), KeybindingContext::Confirmation);
}

#[test]
fn test_shell_prefix_edit_context_and_keys() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    if let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() {
        p.prefix_input = Some(crate::state::PrefixInputState::new("git status:*".into()));
    }

    // Yes row focused (idx 0) → still the y/n/a confirmation context.
    assert_eq!(active_context(&state), KeybindingContext::Confirmation);

    // Focus an allow row (idx 1 = session) → the prefix edit context takes over.
    if let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() {
        p.selected_choice = 1;
    }
    assert_eq!(
        active_context(&state),
        KeybindingContext::PermissionPrefixEdit
    );

    // A letter that would be a y/n/a hotkey inserts text instead.
    assert!(matches!(
        map_key(&state, press(KeyCode::Char('y'))),
        Some(TuiCommand::InsertChar('y'))
    ));
    // Enter commits the focused allow row.
    assert!(matches!(
        map_key(&state, press(KeyCode::Enter)),
        Some(TuiCommand::SurfaceConfirm)
    ));
}

#[test]
fn test_permission_prompt_allow_shortcuts_match_available_actions() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() else {
        panic!("expected permission prompt");
    };
    p.tool_name = "Read".into();
    p.original_input = Some(serde_json::json!({"file_path": "/tmp/project/notes.md"}));

    assert!(matches!(
        map_key(&state, press(KeyCode::Char('a'))),
        Some(TuiCommand::ApproveAll)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Char('s'))),
        Some(TuiCommand::ApproveSession)
    ));
}

#[test]
fn test_permission_session_shortcut_requires_session_action() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);

    assert!(
        map_key(&state, press(KeyCode::Char('s'))).is_none(),
        "s is active only when the current permission prompt has AllowSession"
    );
}

#[test]
fn test_s_is_not_consumed_by_non_permission_confirmation_prompts() {
    let mut state = AppState::new();
    state.ui.push_prompt(PanePromptState::PlanEntry(
        crate::state::PlanEntryPromptState {
            description: "Enter plan mode?".into(),
        },
    ));

    assert!(
        map_key(&state, press(KeyCode::Char('s'))).is_none(),
        "shared confirmation prompts must not consume s as session allow"
    );
}

fn background_tasks_state() -> AppState {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(crate::state::ModalState::BackgroundTasks(
            crate::state::BackgroundTasksState::default(),
        ));
    state
}

#[test]
fn test_shift_up_maps_to_agent_switcher_nav() {
    let state = AppState::new();
    let key = KeyEvent {
        code: KeyCode::Up,
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    assert!(matches!(
        map_key(&state, key),
        Some(TuiCommand::AgentSwitcherNav(-1))
    ));
    // Plain Up stays history-recall, not the switcher.
    assert!(matches!(
        map_key(&state, press(KeyCode::Up)),
        Some(TuiCommand::CursorUp)
    ));
}

#[test]
fn test_background_tasks_uses_dedicated_context() {
    let state = background_tasks_state();
    assert_eq!(active_context(&state), KeybindingContext::BackgroundTasks);
}

#[test]
fn test_background_tasks_nav_keys_reach_the_intercept() {
    // Regression: the dialog routed through the generic Confirmation map, which
    // emits Surface{Prev,Next,Confirm} and has no x/← arms — but
    // `update::background_tasks::intercept` consumes Cursor*/SubmitInput/
    // InsertChar('x'). So ↑/↓/Enter/x/← were all dead and only Esc closed.
    let state = background_tasks_state();
    assert!(matches!(
        map_key(&state, press(KeyCode::Up)),
        Some(TuiCommand::CursorUp)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Down)),
        Some(TuiCommand::CursorDown)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Enter)),
        Some(TuiCommand::SubmitInput)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Left)),
        Some(TuiCommand::CursorLeft)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Esc)),
        Some(TuiCommand::Cancel)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Char('x'))),
        Some(TuiCommand::InsertChar('x'))
    ));
    // Vim-style nav also works.
    assert!(matches!(
        map_key(&state, press(KeyCode::Char('k'))),
        Some(TuiCommand::CursorUp)
    ));
    assert!(matches!(
        map_key(&state, press(KeyCode::Char('j'))),
        Some(TuiCommand::CursorDown)
    ));
    // Reserved exits still win over the dialog map.
    assert!(matches!(
        map_key(&state, ctrl(KeyCode::Char('c'))),
        Some(TuiCommand::Interrupt)
    ));
}

#[test]
fn test_model_picker_context() {
    let state = model_picker_state();
    assert_eq!(active_context(&state), KeybindingContext::ModelPicker);
}

#[test]
fn test_settings_theme_tab_uses_theme_picker_context() {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Settings(
        crate::widgets::settings_panel::SettingsPanelState::new(state.ui.display_settings.clone()),
    ));
    assert_eq!(active_context(&state), KeybindingContext::ThemePicker);
}

#[test]
fn test_theme_picker_ctrl_t_toggles_syntax_highlighting() {
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Settings(
        crate::widgets::settings_panel::SettingsPanelState::new(state.ui.display_settings.clone()),
    ));
    let cmd = map_key(&state, ctrl(KeyCode::Char('t')));
    assert!(matches!(cmd, Some(TuiCommand::ToggleSyntaxHighlighting)));
}

#[test]
fn test_model_picker_left_right_cycle_effort() {
    let state = model_picker_state();
    let left = map_key(&state, press(KeyCode::Left));
    let right = map_key(&state, press(KeyCode::Right));
    assert!(matches!(left, Some(TuiCommand::ModelPickerCycleEffort(-1))));
    assert!(matches!(right, Some(TuiCommand::ModelPickerCycleEffort(1))));
}

#[test]
fn test_model_picker_tab_cycles_role() {
    let state = model_picker_state();
    let cmd = map_key(&state, press(KeyCode::Tab));
    assert!(matches!(cmd, Some(TuiCommand::SettingsNextTab)));
}

#[test]
fn test_copy_picker_w_writes_to_file() {
    let state = copy_picker_state();
    let cmd = map_key(&state, press(KeyCode::Char('w')));
    assert!(matches!(cmd, Some(TuiCommand::CopyPickerWriteToFile)));
}

#[test]
fn test_ctrl_c_interrupts() {
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('c')));
    assert!(matches!(cmd, Some(TuiCommand::Interrupt)));
}

#[test]
fn test_ctrl_d_requests_exit() {
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('d')));
    assert!(matches!(cmd, Some(TuiCommand::RequestExit)));
}

#[test]
fn test_reserved_ctrl_d_wins_in_confirmation_context() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);

    let cmd = map_key(&state, ctrl(KeyCode::Char('d')));

    assert!(matches!(cmd, Some(TuiCommand::RequestExit)));
}

#[test]
fn test_ctrl_q_quits() {
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('q')));
    assert!(matches!(cmd, Some(TuiCommand::Quit)));
}

#[test]
fn test_ctrl_o_toggles_transcript() {
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('o')));
    assert!(matches!(cmd, Some(TuiCommand::ToggleTranscript)));
}

#[test]
fn test_ctrl_shift_o_toggles_teammate_preview() {
    let state = AppState::new();
    let cmd = map_key(&state, ctrl_shift(KeyCode::Char('o')));
    assert!(matches!(
        cmd,
        Some(TuiCommand::ToggleTeammateMessagePreview)
    ));
}

/// End-to-end A7a regression guard: `ctrl+shift+t` resolves through the
/// rebindable `app:toggleTeamRoster` action to `OpenTeamRoster` when a teammate
/// is present. Before A7a the roster picker had NO reachable key — `ctrl+t` was
/// claimed by `app:toggleTodos` and the hardcoded fallback was dead. Without a
/// teammate the key is an inert no-op (not a global shadow).
#[test]
fn test_ctrl_shift_t_opens_team_roster_when_teammate_present() {
    let mut state = AppState::new();
    // No teammate ⇒ inert (the resolver fires the action, dispatch returns None).
    assert!(map_key(&state, ctrl_shift(KeyCode::Char('t'))).is_none());

    state
        .session
        .subagents
        .push(crate::state::SubagentInstance {
            kind: crate::state::SubagentKind::Teammate,
            agent_id: "researcher@my-team".into(),
            agent_type: "explore".into(),
            description: String::new(),
            status: crate::state::SubagentStatus::Running,
            color: None,
            team_name: Some("my-team".into()),
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        });
    let cmd = map_key(&state, ctrl_shift(KeyCode::Char('t')));
    assert!(
        matches!(cmd, Some(TuiCommand::OpenTeamRoster)),
        "ctrl+shift+t with a teammate present must open the roster; got {cmd:?}"
    );
}

/// Inside the roster picker, plain Left/Right cycle the FOCUSED teammate's
/// mode; Shift+Left/Right cycle ALL teammates in tandem (R8 cycle-all).
#[test]
fn test_roster_shift_arrows_cycle_all_modes() {
    let state = team_roster_state();
    assert!(matches!(
        map_key(&state, shift(KeyCode::Right)),
        Some(TuiCommand::TeamRosterCycleAllModes(1))
    ));
    assert!(matches!(
        map_key(&state, shift(KeyCode::Left)),
        Some(TuiCommand::TeamRosterCycleAllModes(-1))
    ));
    // Plain arrows still cycle only the focused member.
    assert!(matches!(
        map_key(&state, press(KeyCode::Right)),
        Some(TuiCommand::TeamRosterCycleMode(1))
    ));
}

#[test]
fn test_enter_submits() {
    let mut state = AppState::new();
    state.ui.input.textarea.insert_str("h");
    let cmd = map_key(&state, press(KeyCode::Enter));
    assert!(matches!(cmd, Some(TuiCommand::SubmitInput)));
}

#[test]
fn test_enter_steers_during_active_turn_over_suggestion() {
    // While a turn is in flight — its whole lifecycle, not just the token
    // stream — Enter must route to SubmitInput so update.rs can queue/steer.
    // Even a visible prompt suggestion must not hijack it. The gate is
    // `turn_active()` (TS `queryGuard.isActive`), NOT `is_streaming()`: here the
    // turn is active but not streaming, and a suggestion is visible, so a
    // regression to the streaming-only gate would yield SubmitPromptSuggestion.
    let mut state = AppState::new();
    state.session.prompt_suggestions = vec!["Run tests".into()];
    state
        .ui
        .ephemeral
        .start_turn("Working", std::time::Instant::now());
    assert!(!state.is_streaming());
    assert!(super::prompt_suggestion_visible(&state));

    let cmd = map_key(&state, press(KeyCode::Enter));
    assert!(matches!(cmd, Some(TuiCommand::SubmitInput)));
}

#[test]
fn test_char_inserts() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::Char('x')));
    assert!(matches!(cmd, Some(TuiCommand::InsertChar('x'))));
}

#[test]
fn test_plain_character_input_is_not_logged_as_key_operation() {
    // Char inserts are always suppressed regardless of context.
    assert!(!should_log_key_command(
        KeybindingContext::Chat,
        &TuiCommand::InsertChar('x')
    ));
    assert!(!should_log_key_command(
        KeybindingContext::Picker,
        &TuiCommand::InsertChar('x')
    ));
    // Meaningful control commands always log.
    assert!(should_log_key_command(
        KeybindingContext::Chat,
        &TuiCommand::SubmitInput
    ));
    assert!(should_log_key_command(
        KeybindingContext::Chat,
        &TuiCommand::Cancel
    ));
}

#[test]
fn test_input_editor_keystrokes_suppressed_in_chat_only() {
    use TuiCommand::*;
    // In Chat (input box), editing keys are noise — drop them.
    for cmd in [
        DeleteBackward,
        DeleteForward,
        CursorLeft,
        CursorRight,
        CursorUp,
        CursorDown,
        WordLeft,
        WordRight,
    ] {
        assert!(
            !should_log_key_command(KeybindingContext::Chat, &cmd),
            "expected suppression in Chat for {cmd:?}"
        );
    }
    // In modal/overlay contexts the SAME keys are control navigation —
    // keep the log.
    for cmd in [DeleteBackward, CursorLeft, CursorRight] {
        assert!(
            should_log_key_command(KeybindingContext::Transcript, &cmd),
            "expected log in Transcript for {cmd:?}"
        );
        assert!(
            should_log_key_command(KeybindingContext::Picker, &cmd),
            "expected log in Picker for {cmd:?}"
        );
    }
}

#[test]
fn test_tab_toggles_plan() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::Tab));
    assert!(matches!(cmd, Some(TuiCommand::TogglePlanMode)));
}

#[test]
fn test_f1_shows_help() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::F(1)));
    assert!(matches!(cmd, Some(TuiCommand::ShowHelp)));
}

#[test]
fn test_esc_cancels() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::Esc));
    assert!(matches!(cmd, Some(TuiCommand::Cancel)));
}

#[test]
fn test_prompt_y_approves() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    let cmd = map_key(&state, press(KeyCode::Char('y')));
    assert!(matches!(cmd, Some(TuiCommand::Approve)));
}

#[test]
fn test_prompt_enter_selects_focused_action() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    let cmd = map_key(&state, press(KeyCode::Enter));
    assert!(matches!(cmd, Some(TuiCommand::SurfaceConfirm)));
}

#[test]
fn test_prompt_n_denies() {
    let mut state = AppState::new();
    install_permission_prompt(&mut state);
    let cmd = map_key(&state, press(KeyCode::Char('n')));
    assert!(matches!(cmd, Some(TuiCommand::Deny)));
}

#[test]
fn test_ctrl_t_cycles_view_in_chat_context() {
    // Ctrl+T binds globally to `app:toggleTodos` (view cycle:
    // Chat → Tasks → Subagents). The previous Chat-context shadow that
    // routed Ctrl+T to `ChatCycleThinking` has moved to Ctrl+Y.
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('t')));
    assert!(matches!(cmd, Some(TuiCommand::ToggleExpandedTasksView)));
}

#[test]
fn test_ctrl_y_cycles_thinking_level_in_chat_context() {
    // In Chat context Ctrl+Y cycles the Main role's thinking effort through
    // the active model's `supported_thinking_levels`. Displaces the readline
    // `yank` default; the legacy `input:yank` cascade only applies in
    // non-Chat contexts.
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('y')));
    assert!(matches!(cmd, Some(TuiCommand::CycleThinkingLevel)));
}

#[test]
fn test_f2_toggles_thinking_display() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::F(2)));

    assert!(matches!(cmd, Some(TuiCommand::ToggleThinking)));
}

#[test]
fn test_pageup_scrolls() {
    let state = AppState::new();
    let cmd = map_key(&state, press(KeyCode::PageUp));
    assert!(matches!(cmd, Some(TuiCommand::PageUp)));
}

#[test]
fn test_ctrl_f_kills_all_agents() {
    // Spec: crate-coco-tui.md §Keyboard Shortcuts — Ctrl+F = kill all agents.
    let state = AppState::new();
    let cmd = map_key(&state, ctrl(KeyCode::Char('f')));
    assert!(matches!(cmd, Some(TuiCommand::KillAllAgents)));
}

#[test]
fn test_ctrl_shift_f_opens_global_search() {
    // `app:globalSearch` is bound to `ctrl+shift+f`.
    let state = AppState::new();
    let key = KeyEvent {
        code: KeyCode::Char('f'),
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    let cmd = map_key(&state, key);
    assert!(matches!(cmd, Some(TuiCommand::ShowGlobalSearch)));
}

#[test]
fn test_autocomplete_context_not_activated_with_empty_items() {
    // Async trigger installed but results haven't arrived yet — items is
    // empty. Arrow keys must keep passing through to input editing so the
    // user can navigate history while search runs.
    let mut state = AppState::new();
    state.ui.completion.active = Some(crate::state::ActiveSuggestions {
        kind: crate::state::SuggestionKind::At,
        items: Vec::new(),
        selected: 0,
        query: "src".into(),
        trigger_pos: 0,
    });
    assert_eq!(active_context(&state), KeybindingContext::Chat);
}

#[test]
fn test_autocomplete_context_when_suggestions_active() {
    // Spec: crate-coco-tui.md §Autocomplete Systems — once suggestions are
    // visible, key dispatch must route Up/Down/Tab/Enter/Esc through the
    // Autocomplete context.
    let mut state = AppState::new();
    state.ui.completion.active = Some(crate::state::ActiveSuggestions {
        kind: crate::state::SuggestionKind::SlashCommand,
        items: vec![crate::widgets::suggestion_popup::SuggestionItem {
            label: "/help".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: String::new(),
        trigger_pos: 0,
    });
    assert_eq!(active_context(&state), KeybindingContext::Autocomplete);

    let tab = map_key(&state, press(KeyCode::Tab));
    assert!(matches!(tab, Some(TuiCommand::AutocompleteAccept)));

    let enter = map_key(&state, press(KeyCode::Enter));
    assert!(matches!(enter, Some(TuiCommand::AutocompleteSubmit)));

    let up = map_key(&state, press(KeyCode::Up));
    assert!(matches!(up, Some(TuiCommand::SurfacePrev)));

    let ctrl_n = map_key(&state, ctrl(KeyCode::Char('n')));
    assert!(matches!(ctrl_n, Some(TuiCommand::SurfaceNext)));

    // Typing a character should fall through to input editing, not be
    // swallowed by the autocomplete context.
    let ch = map_key(&state, press(KeyCode::Char('x')));
    assert!(matches!(ch, Some(TuiCommand::InsertChar('x'))));
}

#[test]
fn test_tab_accepts_inline_ghost_before_plan_toggle() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("then /cl");
    state.ui.input.textarea.set_cursor("then /cl".len());
    state.ui.input.set_inline_ghost(crate::state::InlineGhost {
        text: "ear".into(),
        insert_position: "then /cl".len(),
        replace_start: "then /cl".len(),
        replace_end: "then /cl".len(),
        replacement: "ear".into(),
        cursor_after_accept: "then /clear".len(),
    });

    let tab = map_key(&state, press(KeyCode::Tab));

    assert!(matches!(tab, Some(TuiCommand::AutocompleteAccept)));
}

#[test]
fn test_prompt_suggestion_keys_win_when_input_empty() {
    let mut state = AppState::new();
    state.session.prompt_suggestions = vec!["Run tests".into()];

    let tab = map_key(&state, press(KeyCode::Tab));
    let right = map_key(&state, press(KeyCode::Right));
    let enter = map_key(&state, press(KeyCode::Enter));

    assert!(matches!(tab, Some(TuiCommand::AcceptPromptSuggestion)));
    assert!(matches!(right, Some(TuiCommand::AcceptPromptSuggestion)));
    assert!(matches!(enter, Some(TuiCommand::SubmitPromptSuggestion)));
}

#[test]
fn test_alt_v_pastes() {
    // Spec parity with Ctrl+V — Alt+V also pastes from clipboard.
    let state = AppState::new();
    let key = KeyEvent {
        code: KeyCode::Char('v'),
        modifiers: KeyModifiers::ALT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    let cmd = map_key(&state, key);
    assert!(matches!(cmd, Some(TuiCommand::PasteFromClipboard)));
}

#[test]
fn ctrl_s_and_ctrl_g_resolve_to_restored_actions_not_literal_inserts() {
    // RC-4 (H1+M6): the cascade deletion left ctrl+s / ctrl+g with no binding,
    // so they degraded to InsertChar('s')/('g') in the composer. They are
    // restored as the rebindable app:sessionBrowser / app:planEditor actions.
    let state = AppState::new();
    assert!(matches!(
        map_key(&state, ctrl(KeyCode::Char('s'))),
        Some(TuiCommand::ShowSessionBrowser)
    ));
    assert!(matches!(
        map_key(&state, ctrl(KeyCode::Char('g'))),
        Some(TuiCommand::OpenPlanEditor)
    ));
}

#[test]
fn unbound_ctrl_combo_is_swallowed_not_inserted() {
    // RC-4 (H1): a Ctrl+<char> that reaches the input cascade with no binding
    // must NOT type its literal letter into the composer (the regression that
    // corrupted input in Chat and under the autocomplete popup).
    let state = AppState::new();
    assert!(
        !matches!(
            map_key(&state, ctrl(KeyCode::Char('.'))),
            Some(TuiCommand::InsertChar(_))
        ),
        "an unbound Ctrl combo must not insert a literal char",
    );
}
