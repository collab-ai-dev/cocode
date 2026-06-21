use super::dispatch_action;
use crate::events::TuiCommand;
use crate::state::AppState;
use coco_keybindings::KeybindingAction;

fn fresh_state() -> AppState {
    AppState::default()
}

#[test]
fn app_interrupt_maps_to_interrupt() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::AppInterrupt, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::Interrupt));
}

#[test]
fn chat_submit_routes_to_update_layer_when_streaming() {
    let mut state = fresh_state();
    state.ui.streaming = Some(crate::state::StreamingState::default());
    let cmd = dispatch_action(&KeybindingAction::ChatSubmit, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::SubmitInput));
}

#[test]
fn chat_submit_submits_when_idle() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::ChatSubmit, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::SubmitInput));
}

#[test]
fn confirm_yes_maps_to_approve() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::ConfirmYes, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::Approve));
}

#[test]
fn select_accept_maps_to_surface_confirm() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::SelectAccept, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::SurfaceConfirm));
}

#[test]
fn command_action_synthesizes_slash_command() {
    let state = fresh_state();
    // `command:help` user-binding → submit `/help` through the same
    // slash-command runner the agent driver uses.
    let cmd = dispatch_action(&KeybindingAction::Command("help".into()), &state).unwrap();
    match cmd {
        TuiCommand::ExecuteSlashCommand(name) => assert_eq!(name, "help"),
        other => panic!("expected ExecuteSlashCommand(\"help\"), got {other:?}"),
    }
}

#[test]
fn chat_stash_maps_to_stash_input_draft() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::ChatStash, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::StashInputDraft));
}

#[test]
fn app_toggle_todos_maps_to_toggle_expanded_tasks_view() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::AppToggleTodos, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::ToggleExpandedTasksView));
}

/// `app:toggleTeamRoster` (the new `ctrl+shift+t` binding) is gated on the
/// session actually having a teammate — in a non-team session it returns
/// `None` (inert, no global shadow), and opens the roster when a teammate is
/// present. Regression guard for A7a: the roster picker previously had NO
/// reachable trigger because the resolver claimed `ctrl+t` for todos before
/// the dead hardcoded fallback.
#[test]
fn app_toggle_team_roster_is_inert_without_a_teammate() {
    let state = fresh_state();
    assert!(
        dispatch_action(&KeybindingAction::AppToggleTeamRoster, &state).is_none(),
        "no teammate ⇒ binding must be an inert no-op, not a global shadow"
    );
}

#[test]
fn app_toggle_team_roster_opens_when_a_teammate_is_present() {
    let mut state = fresh_state();
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
    let cmd = dispatch_action(&KeybindingAction::AppToggleTeamRoster, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::OpenTeamRoster));
}

#[test]
fn app_toggle_transcript_maps_to_toggle_transcript() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::AppToggleTranscript, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::ToggleTranscript));
}

#[test]
fn scroll_actions_route_to_transcript_commands_inside_transcript_modal() {
    let mut state = fresh_state();
    state.ui.show_modal(crate::state::ModalState::Transcript(
        crate::state::transcript::TranscriptState::new(),
    ));

    let line = dispatch_action(&KeybindingAction::ScrollLineDown, &state).unwrap();
    assert!(matches!(line, TuiCommand::TranscriptScrollLines(1)));

    let page = dispatch_action(&KeybindingAction::ScrollPageUp, &state).unwrap();
    assert!(matches!(page, TuiCommand::TranscriptPage(-1)));

    let top = dispatch_action(&KeybindingAction::ScrollTop, &state).unwrap();
    assert!(matches!(top, TuiCommand::TranscriptJumpStart));
}

#[test]
fn app_toggle_teammate_preview_maps_to_toggle_teammate_message_preview() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::AppToggleTeammatePreview, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::ToggleTeammateMessagePreview));
}

#[test]
fn feature_gated_actions_silently_no_op() {
    // When a feature isn't ported, the action returns None so the
    // keystroke is swallowed silently.
    let state = fresh_state();
    let gated = [
        KeybindingAction::ChatUndo,
        KeybindingAction::ChatMessageActions,
        KeybindingAction::AppToggleBrief,
        KeybindingAction::AppToggleTerminal,
        KeybindingAction::PermissionToggleDebug,
        KeybindingAction::AttachmentsRemove,
        KeybindingAction::PluginToggle,
        KeybindingAction::PluginInstall,
        KeybindingAction::VoicePushToTalk,
        KeybindingAction::SettingsSearch,
        KeybindingAction::SettingsRetry,
        KeybindingAction::MessageActionsPrev,
        KeybindingAction::MessageActionsNext,
        KeybindingAction::MessageActionsEnter,
    ];
    for action in gated {
        assert!(
            dispatch_action(&action, &state).is_none(),
            "{action:?} should return None — feature-gated action with no surface yet",
        );
    }
}

#[test]
fn theme_toggle_syntax_highlighting_maps_to_toggle_command() {
    let state = fresh_state();
    let cmd = dispatch_action(&KeybindingAction::ThemeToggleSyntaxHighlighting, &state).unwrap();
    assert!(matches!(cmd, TuiCommand::ToggleSyntaxHighlighting));
}
