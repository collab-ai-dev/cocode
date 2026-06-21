//! Tests for TUI AppState.

use std::time::Instant;

use coco_types::PermissionMode;

use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::PlanEntryPromptState;
use crate::state::StreamingState;
use crate::state::UiAnimation;
use crate::state::session::SubagentInstance;
use crate::state::session::SubagentKind;
use crate::state::session::SubagentStatus;
use crate::state::session::TokenUsage;
use crate::state::ui::Toast;
use crate::transcript::cells::CellKind;
use crate::transcript::derive::test_helpers;

// ── Permission-mode cycling ──

/// With neither bypass nor auto available, the cycle is
/// `Default → AcceptEdits → Plan → Default`.
#[test]
fn test_cycle_permission_mode_baseline() {
    let mut state = AppState::new();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::AcceptEdits);

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Plan);

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
}

#[test]
fn test_cycle_permission_mode_with_bypass() {
    let mut state = AppState::new();
    state.session.bypass_permissions_available = true;
    state.session.permission_mode = PermissionMode::Plan;

    state.cycle_permission_mode();
    assert_eq!(
        state.session.permission_mode,
        PermissionMode::BypassPermissions
    );

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
}

#[test]
fn test_cycle_permission_mode_with_auto() {
    let mut state = AppState::new();
    state.session.auto_mode_available = true;
    state.session.permission_mode = PermissionMode::Plan;

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Auto);

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
}

#[test]
fn test_toggle_plan_mode_flips_between_plan_and_default() {
    let mut state = AppState::new();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);

    state.toggle_plan_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Plan);
    assert!(state.is_plan_mode());

    state.toggle_plan_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
    assert!(!state.is_plan_mode());
}

#[test]
fn test_cycle_skips_plan_when_plan_mode_unavailable() {
    // `features.plan_mode = false` → AcceptEdits skips Plan straight to
    // Default (bypass/auto also off), and Tab can't enter plan mode.
    let mut state = AppState::new();
    state.session.plan_mode_available = false;

    state.cycle_permission_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::AcceptEdits);

    state.cycle_permission_mode();
    assert_eq!(
        state.session.permission_mode,
        PermissionMode::Default,
        "Plan must be skipped when the feature is off"
    );

    // Tab is a no-op when plan mode is unavailable.
    state.toggle_plan_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
    assert!(!state.is_plan_mode());
}

#[test]
fn test_toggle_plan_mode_from_accept_edits_goes_to_plan() {
    // Quick toggle is "enable plan mode" regardless of source; it
    // doesn't try to preserve the prior mode.
    let mut state = AppState::new();
    state.session.permission_mode = PermissionMode::AcceptEdits;

    state.toggle_plan_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Plan);

    state.toggle_plan_mode();
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
}

#[test]
fn test_new_state_defaults() {
    let state = AppState::new();
    assert!(!state.should_exit());
    assert!(!state.has_active_surface());
    assert!(!state.is_streaming());
    assert!(!state.should_show_spinner());
    assert!(!state.is_plan_mode());
    assert!(!state.session.fast_mode);
    assert_eq!(state.session.turn_count, 0);
    assert!(!state.ui.ephemeral.turn_active());
    assert!(state.session.transcript.is_empty());
}

#[test]
fn test_apply_display_settings_updates_show_thinking_default() {
    let mut state = AppState::new();
    assert!(!state.ui.show_thinking);

    state.ui.apply_display_settings(crate::DisplaySettings {
        show_thinking: true,
        ..crate::DisplaySettings::default()
    });

    assert!(state.ui.show_thinking);
}

#[test]
fn test_apply_display_settings_keeps_runtime_show_thinking_toggle_when_default_unchanged() {
    let mut state = AppState::new();
    state.ui.apply_display_settings(crate::DisplaySettings {
        show_thinking: false,
        ..crate::DisplaySettings::default()
    });

    state.ui.show_thinking = true;
    state.ui.apply_display_settings(crate::DisplaySettings {
        show_thinking: false,
        ..crate::DisplaySettings::default()
    });

    assert!(state.ui.show_thinking);
}

#[test]
fn test_modal_queue_same_priority_fifo() {
    let mut state = AppState::new();

    // Help (priority 8) installed
    state.ui.show_modal(ModalState::Help);
    assert!(state.has_active_surface());

    // A second Help queues behind (same priority keeps insertion order)
    state.ui.show_modal(ModalState::Help);
    assert!(matches!(state.ui.modal, Some(ModalState::Help)));
    assert_eq!(state.ui.modal_queue.len(), 1);

    // Dismiss promotes the queued one
    state.ui.dismiss_modal();
    assert!(matches!(state.ui.modal, Some(ModalState::Help)));
    assert_eq!(state.ui.modal_queue.len(), 0);

    state.ui.dismiss_modal();
    assert!(!state.has_active_surface());
}

#[test]
fn test_modal_higher_priority_displaces_current() {
    let mut state = AppState::new();

    // Help is the weakest priority (8)
    state.ui.show_modal(ModalState::Help);
    // Error (priority 4) arriving should displace Help back into the queue
    state.ui.show_modal(ModalState::Error("boom".to_string()));
    assert!(matches!(state.ui.modal, Some(ModalState::Error(_))));
    assert_eq!(state.ui.modal_queue.len(), 1);

    // Dismissing Error restores Help
    state.ui.dismiss_modal();
    assert!(matches!(state.ui.modal, Some(ModalState::Help)));
    assert_eq!(state.ui.modal_queue.len(), 0);
}

#[test]
fn test_surface_lower_priority_queues_behind() {
    let mut state = AppState::new();

    // Error (priority 4) is current
    state.ui.show_modal(ModalState::Error("boom".to_string()));
    // Help (priority 8) queues behind without displacing
    state.ui.show_modal(ModalState::Help);
    assert!(matches!(state.ui.modal, Some(ModalState::Error(_))));
    assert_eq!(state.ui.modal_queue.len(), 1);

    state.ui.dismiss_modal();
    assert!(matches!(state.ui.modal, Some(ModalState::Help)));
}

#[test]
fn test_prompt_queue_priority_ordered() {
    let mut state = AppState::new();
    // Install highest-priority prompt so queue fills with strictly lower ones.
    state.ui.push_prompt(PanePromptState::SandboxPermission(
        crate::state::SandboxPermissionPromptState {
            request_id: "r0".into(),
            description: "sandbox".into(),
        },
    ));

    // Enqueue in reverse priority order; insertion should re-sort.
    state.ui.push_prompt(PanePromptState::Question(
        crate::state::QuestionPromptState {
            request_id: "question-1".into(),
            original_input: serde_json::json!({}),
            questions: vec![],
            current_question: crate::state::QuestionPage::Question(0),
            focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
            is_in_plan_mode: false,
        },
    )); // priority 2
    state.ui.push_prompt(PanePromptState::PlanEntry(
        crate::state::PlanEntryPromptState {
            description: "plan-a".into(),
        },
    )); // priority 1
    state.ui.push_prompt(PanePromptState::PlanEntry(
        crate::state::PlanEntryPromptState {
            description: "plan-b".into(),
        },
    )); // priority 1

    // Queue promotes by priority asc after the active sandbox prompt is dismissed.
    let mut priorities = Vec::new();
    for _ in 0..3 {
        state.ui.dismiss_prompt();
        let prompt = state
            .ui
            .interaction
            .active_prompt
            .as_ref()
            .expect("queued prompt should be promoted");
        priorities.push(prompt.priority());
    }
    assert_eq!(priorities, vec![1, 1, 2]);
}

#[test]
fn test_toast_lifecycle() {
    let mut state = AppState::new();

    state.ui.add_toast(Toast::info("hello"));
    assert!(state.ui.has_toasts());
    assert_eq!(state.ui.toasts.len(), 1);

    // Toasts should not be expired immediately
    state.ui.expire_toasts();
    assert!(state.ui.has_toasts());
}

#[test]
fn test_session_messages() {
    let mut state = AppState::new();

    test_helpers::push_user_text(&mut state.session, "1", "hello");
    test_helpers::push_assistant_text(&mut state.session, "hi there");

    let cells = state.session.transcript.cells();
    assert_eq!(cells.len(), 2);
    assert!(matches!(
        cells.last().map(|c| &c.kind),
        Some(CellKind::AssistantText { .. })
    ));
}

#[test]
fn test_tool_execution_lifecycle() {
    let mut state = AppState::new();

    state.session.start_tool(
        "call-1".to_string(),
        "Bash".to_string(),
        &serde_json::json!({ "command": "ls" }),
    );
    assert_eq!(state.session.tool_executions.len(), 1);
    assert_eq!(
        state.session.tool_executions[0].status,
        crate::state::session::ToolStatus::Queued
    );

    state.session.run_tool("call-1");
    assert_eq!(
        state.session.tool_executions[0].status,
        crate::state::session::ToolStatus::Running
    );

    state.session.complete_tool("call-1", /*is_error*/ false);
    assert_eq!(
        state.session.tool_executions[0].status,
        crate::state::session::ToolStatus::Completed
    );

    state.session.complete_tool("call-2", /*is_error*/ true);
    // Non-existent tool: no panic
}

#[test]
fn test_streaming_tool_upgrades_in_place_no_duplicate() {
    use crate::state::session::ToolStatus;
    let mut state = AppState::new();

    // Args start streaming: a `Streaming` row opens.
    state
        .session
        .begin_tool_stream("c1".to_string(), "Bash".to_string());
    // Duplicate stream-start for the same call_id is a no-op.
    state
        .session
        .begin_tool_stream("c1".to_string(), "Bash".to_string());
    assert_eq!(state.session.tool_executions.len(), 1);
    assert_eq!(
        state.session.tool_executions[0].status,
        ToolStatus::Streaming
    );

    // Partial argument JSON arrives → live preview reflects what's typed.
    state
        .session
        .append_tool_stream_delta("c1", r#"{"command":"cargo bui"#);
    assert_eq!(
        state.session.tool_executions[0].input_preview.as_deref(),
        Some("cargo bui")
    );

    // Input finalizes (`ToolUseQueued`): the SAME row upgrades in place —
    // Streaming → Queued, partial preview replaced by the finalized one, no
    // duplicate row, partial buffer cleared.
    state.session.start_tool(
        "c1".to_string(),
        "Bash".to_string(),
        &serde_json::json!({ "command": "cargo build" }),
    );
    assert_eq!(state.session.tool_executions.len(), 1);
    assert_eq!(state.session.tool_executions[0].status, ToolStatus::Queued);
    assert_eq!(
        state.session.tool_executions[0].input_preview.as_deref(),
        Some("cargo build")
    );
    assert!(state.session.tool_executions[0].streaming_input.is_none());
}

#[test]
fn test_begin_tool_stream_suppresses_overlay_tools() {
    let mut state = AppState::new();
    state
        .session
        .begin_tool_stream("c1".to_string(), "ExitPlanMode".to_string());
    // Overlay-driven tools own a dedicated surface — no streaming row leaks.
    assert!(state.session.tool_executions.is_empty());
}

#[test]
fn test_input_editing() {
    let mut state = AppState::new();

    state.ui.input.textarea.insert_str("h");
    state.ui.input.textarea.insert_str("i");
    assert_eq!(state.ui.input.text(), "hi");
    assert_eq!(state.ui.input.textarea.cursor(), 2);

    state.ui.input.textarea.move_cursor_left();
    assert_eq!(state.ui.input.textarea.cursor(), 1);

    state.ui.input.textarea.insert_str("!");
    assert_eq!(state.ui.input.text(), "h!i");

    let taken = state.ui.input.take_input();
    assert_eq!(taken, "h!i");
    assert!(state.ui.input.is_empty());
    assert_eq!(state.ui.input.textarea.cursor(), 0);
}

#[test]
fn test_input_history_most_recent_first() {
    let mut state = AppState::new();

    state.ui.input.add_to_history("first".to_string());
    state.ui.input.add_to_history("second".to_string());
    assert_eq!(state.ui.input.history.len(), 2);
    // Newest submission sits at the front for up-arrow recall.
    assert_eq!(state.ui.input.history[0].text, "second");
    assert_eq!(state.ui.input.history[1].text, "first");

    // Re-submitting an existing entry moves it to the front without
    // creating a duplicate.
    state.ui.input.add_to_history("first".to_string());
    assert_eq!(state.ui.input.history.len(), 2);
    assert_eq!(state.ui.input.history[0].text, "first");
    assert_eq!(state.ui.input.history[1].text, "second");
}

#[test]
fn test_hydrate_history_dedups_newest_first() {
    let mut state = AppState::new();

    // `get_history` yields newest-first, possibly with duplicate display
    // text across sessions; hydration keeps the first (newest) occurrence.
    state.ui.input.hydrate_history(vec![
        "newest".to_string(),
        "older".to_string(),
        "newest".to_string(),
        String::new(),
    ]);

    assert_eq!(state.ui.input.history.len(), 2);
    assert_eq!(state.ui.input.history[0].text, "newest");
    assert_eq!(state.ui.input.history[1].text, "older");
}

#[test]
fn test_permission_prompt() {
    let mut state = AppState::new();

    state
        .ui
        .push_prompt(PanePromptState::Permission(PermissionPromptState {
            request_id: "req-1".to_string(),
            tool_name: "Bash".to_string(),
            description: "Run command".to_string(),
            detail: PermissionDetail::Bash {
                command: "ls -la".to_string(),
                risk_description: None,
                working_dir: None,
            },
            risk_level: None,
            show_always_allow: true,
            classifier_checking: false,
            classifier_auto_approved: None,
            choices: None,
            selected_choice: 0,
            display_input: coco_types::PermissionDisplayInput::Command("rm -rf /tmp/test".into()),
            original_input: None,
            cwd: None,
            permission_suggestions: vec![],
            worker_badge: None,
            explanation_visible: false,
            explanation: crate::state::ExplainerFetch::NotFetched,
            prefix_input: None,
        }));

    assert!(state.has_active_surface());
    assert!(matches!(
        state.ui.interaction.active_prompt,
        Some(PanePromptState::Permission(_))
    ));
}

#[test]
fn test_streaming_state() {
    let mut state = AppState::new();
    assert!(!state.is_streaming());

    state.ui.streaming = Some(crate::state::ui::StreamingState::new());
    assert!(state.is_streaming());
    assert!(state.should_show_spinner());

    if let Some(ref mut s) = state.ui.streaming {
        s.append_text("hello ");
        s.append_text("world\n");
        assert_eq!(s.content, "hello world\n");
        assert_eq!(s.visible_content(), ""); // cursor at 0

        s.advance_display();
        assert_eq!(s.visible_content(), "hello world\n");
    }
}

#[test]
fn test_token_usage_update() {
    let mut state = AppState::new();

    state.session.update_tokens(TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        reasoning_tokens: 0,
        cache_read_tokens: 20,
        cache_creation_tokens: 10,
    });

    assert_eq!(state.session.token_usage.input_tokens, 100);
    assert_eq!(state.session.token_usage.output_tokens, 50);
}

fn running_subagent(status: SubagentStatus) -> SubagentInstance {
    SubagentInstance {
        kind: SubagentKind::Subagent,
        agent_id: "agent-1".into(),
        agent_type: "Explore".into(),
        description: "scan".into(),
        status,
        color: None,
        team_name: None,
        started_at_ms: Some(0),
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
    }
}

// ── Frame-animation gating (`AppState::ui_animation`) ──

#[test]
fn test_ui_animation_idle_when_session_quiescent() {
    let state = AppState::new();
    assert_eq!(state.ui_animation(), UiAnimation::Idle);
}

#[test]
fn test_ui_animation_stream_reveal_while_streaming() {
    let mut state = AppState::new();
    state.ui.streaming = Some(StreamingState::default());
    assert_eq!(state.ui_animation(), UiAnimation::StreamReveal);
}

#[test]
fn test_ui_animation_spinner_while_turn_running() {
    let mut state = AppState::new();
    state.ui.ephemeral.start_turn("Thinking", Instant::now());
    assert_eq!(state.ui_animation(), UiAnimation::SpinnerOnly);
}

#[test]
fn test_ui_animation_spinner_while_compacting() {
    let mut state = AppState::new();
    state.session.is_compacting = true;
    assert_eq!(state.ui_animation(), UiAnimation::SpinnerOnly);
}

/// Regression for the 12 fps idle-redraw bug: a turn paused on a
/// blocking tool-permission prompt must NOT keep animating. The clock
/// freezes, the glyph (a pure function of `elapsed_ms`) freezes with it,
/// so the old self-arm re-painted an identical frame ~12×/s for the whole
/// wait. The two observed multi-minute gaps were both pausing prompts.
#[test]
fn test_ui_animation_idle_while_turn_paused_on_blocking_prompt() {
    let mut state = AppState::new();
    let now = Instant::now();
    state.ui.ephemeral.start_turn("Thinking", now);
    // What a `Permission` / `SandboxPermission` prompt does to the turn.
    state.ui.ephemeral.tick_pause_clock(/*blocked*/ true, now);
    assert_eq!(state.ui_animation(), UiAnimation::Idle);
}

/// A *non-pausing* prompt (PlanEntry/Question/…) leaves the status
/// indicator visible with a running clock, so the spinner must stay
/// armed. Over-suppressing here would freeze a visibly-ticking spinner.
#[test]
fn test_ui_animation_spinner_during_non_pausing_prompt() {
    let mut state = AppState::new();
    state.ui.ephemeral.start_turn("Thinking", Instant::now());
    state
        .ui
        .push_prompt(PanePromptState::PlanEntry(PlanEntryPromptState {
            description: "plan".into(),
        }));
    assert_eq!(state.ui_animation(), UiAnimation::SpinnerOnly);
}

/// A full-screen modal covers the interactive viewport, so the spinner is
/// not painted and must not be armed even while a turn runs underneath.
#[test]
fn test_ui_animation_idle_while_modal_covers_running_turn() {
    let mut state = AppState::new();
    state.ui.ephemeral.start_turn("Thinking", Instant::now());
    state.ui.show_modal(ModalState::Help);
    assert_eq!(state.ui_animation(), UiAnimation::Idle);
}

/// A subagent running *outside* an active foreground turn (swarm teammate,
/// `run_in_background`, the inter-turn gap) carries a live elapsed clock in
/// the Agents panel, so the spinner cadence must stay armed — otherwise the
/// frame scheduler sleeps and the clock freezes until the next event, then
/// jumps. Regression for the "整个UI看起来没有刷新" freeze.
#[test]
fn test_ui_animation_spinner_while_subagent_runs_without_turn() {
    let mut state = AppState::new();
    assert!(!state.ui.ephemeral.turn_active());
    state
        .session
        .subagents
        .push(running_subagent(SubagentStatus::Running));
    assert_eq!(state.ui_animation(), UiAnimation::SpinnerOnly);
}

/// Once every subagent has finished, the panel rows freeze at their terminal
/// elapsed and no longer need a live clock — the animation must idle so the
/// scheduler can sleep.
#[test]
fn test_ui_animation_idle_when_subagents_all_finished() {
    let mut state = AppState::new();
    state
        .session
        .subagents
        .push(running_subagent(SubagentStatus::Completed));
    assert_eq!(state.ui_animation(), UiAnimation::Idle);
}

/// Streaming takes precedence over a pending prompt: the reveal keeps
/// producing visible rows, so it must stay armed.
#[test]
fn test_ui_animation_stream_reveal_overrides_prompt() {
    let mut state = AppState::new();
    state.ui.streaming = Some(StreamingState::default());
    state
        .ui
        .push_prompt(PanePromptState::PlanEntry(PlanEntryPromptState {
            description: "plan".into(),
        }));
    assert_eq!(state.ui_animation(), UiAnimation::StreamReveal);
}
